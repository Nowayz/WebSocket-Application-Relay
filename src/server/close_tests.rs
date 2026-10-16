//! Close output must survive real TCP backpressure after the parser receives
//! the peer's Close. No application or decoder state may discard the reply.
use super::*;
use crate::Config;
use tungstenite::protocol::Role;

#[test]
fn a_blocked_close_acknowledgement_waits_for_output_without_reading_a_closed_decoder() {
    exercise_blocked_close(false);
}

#[test]
fn a_blocked_server_close_survives_the_peers_acknowledgement_before_output_finishes() {
    exercise_blocked_close(true);
}

fn exercise_blocked_close(server_initiated: bool) {
    let relay = Relay::new(Config {
        ping_interval_secs: 0,
        total_read_buffer_bytes: 0,
        ..Config::default()
    })
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let raw_client = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .unwrap();
    raw_client.set_recv_buffer_size(1024).unwrap();
    raw_client
        .connect(&listener.local_addr().unwrap().into())
        .unwrap();
    let (server, _) = listener.accept().unwrap();
    server.set_nodelay(true).unwrap();
    server.set_nonblocking(true).unwrap();
    socket2::SockRef::from(&server)
        .set_send_buffer_size(4096)
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = Worker::new(listener, relay.clone()).unwrap();
    relay.metrics.connections.fetch_add(1, Relaxed);
    let socket = Socket::new(
        mio::net::TcpStream::from_std(server),
        Token(2),
        worker.registry.clone(),
        worker.signal.clone(),
        ConnectionGuard(relay.metrics.clone(), None),
        &relay,
    )
    .unwrap();
    let mut connection = Connection::new(socket.clone(), Token(2), None, &relay);
    connection.stage = Stage::WebSocket(WebSocket::from_raw_socket(
        Stream {
            socket: socket.clone(),
            remaining_upgrade: None,
        },
        Role::Server,
        Some(
            WebSocketConfig::default()
                .read_buffer_size(4096)
                .shared_read_buffer(true)
                .write_buffer_size(4096)
                .max_write_buffer_size(relay.config.queue_bytes + 4096),
        ),
    ));
    connection.registration = Some(
        relay
            .subscribe_with_output(b"blocked-close", socket.clone())
            .unwrap(),
    );
    let raw_client: std::net::TcpStream = raw_client.into();
    raw_client
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    let mut client = WebSocket::from_raw_socket(raw_client, Role::Client, None);
    assert!(matches!(client.read().unwrap(), Message::Binary(_)));

    // The real socket has a small receive window and send buffer. Fill it with
    // Pong output, as when a peer sends Pings without reading the replies. The
    // codec keeps any short-write remainder intact.
    let pong = Bytes::from_static(&[0x5a; 125]);
    for _ in 0..3 {
        let Stage::WebSocket(ws) = &mut connection.stage else {
            unreachable!()
        };
        let mut blocked = 0;
        for _ in 0..1000 {
            match ws.send(Message::Pong(pong.clone())) {
                Ok(()) => {}
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    blocked += 1;
                    if blocked == 128 {
                        break;
                    }
                }
                result => panic!("unexpected fill result: {result:?}"),
            }
        }
        assert_eq!(blocked, 128, "test socket did not sustain backpressure");
        std::thread::sleep(Duration::from_millis(20));
    }
    let Stage::WebSocket(ws) = &mut connection.stage else {
        unreachable!()
    };
    assert!(
        matches!(ws.flush(), Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock)
    );

    if server_initiated {
        assert!(
            connection.start_close(Disconnect::IDLE, &relay, false, Instant::now()) != Drive::Drop
        );
    }
    client.close(None).unwrap();
    let mut scratch = ReadStorage::default();
    assert!(connection.drive(&relay, &mut scratch, false) != Drive::Drop);
    assert_eq!(
        relay.counts(),
        (0, 1),
        "membership retires before Close output"
    );
    assert!(connection.closing.as_ref().unwrap().flush_only);
    assert!(
        matches!(connection.drive(&relay, &mut scratch, false), Drive::Wait),
        "a blocked acknowledgement must not read/terminate the closed decoder"
    );

    // Release the deliberately tiny window before draining. Keeping a window
    // smaller than loopback's MSS would measure TCP persist delays instead of
    // whether the pending Close resumes when backpressure clears.
    socket2::SockRef::from(client.get_ref())
        .set_recv_buffer_size(1024 * 1024)
        .unwrap();
    let reader = std::thread::spawn(move || {
        loop {
            match client.read().unwrap() {
                Message::Pong(bytes) => assert_eq!(bytes, pong),
                Message::Close(frame) => {
                    if server_initiated {
                        let frame = frame.expect("server's original Close");
                        assert_eq!(u16::from(frame.code), Disconnect::IDLE.code);
                        assert_eq!(frame.reason, Disconnect::IDLE.reason);
                    } else {
                        assert!(frame.is_none());
                    }
                    return;
                }
                other => panic!("unexpected output: {other:?}"),
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(4);
    while connection.drive(&relay, &mut scratch, false) != Drive::Drop {
        assert!(Instant::now() < deadline, "Close output failed to resume");
        std::thread::yield_now();
    }
    drop(connection);
    drop(socket);
    reader.join().unwrap();
    assert_eq!(relay.metrics.close_failures.load(Relaxed), 0);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
}
