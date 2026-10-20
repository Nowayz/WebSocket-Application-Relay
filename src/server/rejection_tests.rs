use super::*;
use tungstenite::protocol::Role;

#[derive(Clone, Copy)]
enum Failure {
    Malformed,
    Unauthorized,
    Staging,
    Partial,
    ClosingPrefix,
}

#[test]
fn routing_validation_and_permission_failures_charge_one_attempt() {
    for failure in [Failure::Malformed, Failure::Unauthorized] {
        for allowance in [1, 17, FANOUT_QUANTUM * 4] {
            check(failure, allowance);
        }
    }
}

#[test]
fn routing_overload_charges_refusals_and_attempted_partial_fanout() {
    for failure in [Failure::Partial, Failure::Staging, Failure::ClosingPrefix] {
        for allowance in [1, 17, FANOUT_QUANTUM * 4] {
            check(failure, allowance);
        }
    }
}

fn check(failure: Failure, allowance: usize) {
    let relay = Relay::new(crate::Config {
        auth_password: Some("reject".into()),
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: 16384,
        ping_interval_secs: 0,
        total_read_buffer_bytes: 0,
        ..crate::Config::default()
    })
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = Worker::new(listener, relay.clone()).unwrap();
    relay.metrics.connections.fetch_add(1, Relaxed);
    let socket = Socket::new(
        mio::net::TcpStream::from_std(stream),
        Token(2),
        worker.registry.clone(),
        worker.signal.clone(),
        ConnectionGuard(relay.metrics.clone(), None),
        &relay,
    )
    .unwrap();
    let mut connection = Connection::new(socket.clone(), Token(2), None, &relay);
    connection.registration = Some(
        relay
            .subscribe_with_output(b"reject-source", socket.clone())
            .unwrap(),
    );
    let source = connection.registration.as_ref().unwrap().peer.clone();
    let count = match failure {
        Failure::Staging => FANOUT_QUANTUM * 2,
        Failure::Partial | Failure::ClosingPrefix => FANOUT_QUANTUM + 44,
        _ => 0,
    };
    let mut observers = Vec::new();
    for _ in 0..count {
        let mut observer = relay.subscribe(protocol::GLOBAL_CHANNEL).unwrap();
        drop(observer.outgoing.try_recv().unwrap());
        for request in [
            protocol::request(0, &protocol::text("reject")),
            protocol::request(1, b"\x02"),
        ] {
            relay.receive(&observer.registration.peer, request).unwrap();
            drop(observer.outgoing.try_recv().unwrap());
        }
        observers.push(observer);
    }
    let mut held = Vec::new();
    let mut fillers = Vec::new();
    if matches!(failure, Failure::ClosingPrefix) {
        // Logical retirement can lag a close request. Count these visited
        // handles even though their output is skipped and consumes no credit.
        for (index, observer) in observers.iter().enumerate() {
            if index != 1000 {
                observer.registration.peer.close(Disconnect::SHUTDOWN);
            }
        }
        for _ in 0..8 {
            let mut filler = relay.subscribe(b"reject-fill").unwrap();
            drop(filler.outgoing.try_recv().unwrap());
            fillers.push(filler);
        }
        relay
            .receive(
                &fillers[0].registration.peer,
                protocol::envelope(protocol::BROADCAST, &protocol::byte_string(&[0x5a; 1000])),
            )
            .unwrap();
        for filler in &mut fillers {
            while let Ok(packet) = filler.outgoing.try_recv() {
                held.push(packet);
            }
        }
        assert_eq!(held.len(), 7);
    }
    let value = protocol::byte_string(&[0x35; 256]);
    let body = match failure {
        Failure::Malformed => Bytes::from_static(b"\xf5"),
        Failure::Unauthorized => protocol::request(1, b"\x02"),
        _ => protocol::envelope(source.id.load(Relaxed), &value),
    };
    let denied = protocol::envelope(source.id.load(Relaxed), b"\xf4");
    let mut input = Vec::new();
    for body in [&body, &denied] {
        // Both frames are present before driving; rejection must not read ahead.
        let len = body.len();
        input.push(0x82);
        if len < 126 {
            input.push(0x80 | len as u8);
        } else {
            input.push(0xfe);
            input.extend_from_slice(&(len as u16).to_be_bytes());
        }
        input.extend_from_slice(&[0; 4]);
        input.extend_from_slice(body);
    }
    connection.stage = Stage::WebSocket(WebSocket::from_partially_read(
        Stream {
            socket: socket.clone(),
            remaining_upgrade: None,
        },
        input,
        Role::Server,
        Some(
            WebSocketConfig::default()
                .read_buffer_size(4096)
                .shared_read_buffer(true),
        ),
    ));
    let before = relay.metrics.incoming_messages.load(Relaxed);
    let mut remaining = allowance;
    assert!(matches!(
        connection.drive_batched(
            &relay,
            &mut ReadStorage::default(),
            false,
            &mut relay.receive_batch(),
            &mut remaining,
        ),
        Drive::Again | Drive::Drop
    ));
    let mut packets = Vec::new();
    for observer in &mut observers {
        while let Ok(packet) = observer.outgoing.try_recv() {
            assert_eq!(
                packet.frame.to_vec(),
                protocol::envelope(source.id.load(Relaxed), &value)
            );
            packets.push(packet);
        }
    }
    if matches!(failure, Failure::Partial) {
        assert!(
            !packets.is_empty() && packets.len() < FANOUT_QUANTUM,
            "diagnostic did not fail after a real partial native turn"
        );
    } else {
        assert!(packets.is_empty());
    }
    let charge = if matches!(failure, Failure::Partial) {
        // The primary visit, submitted observer prefix and refused observer.
        packets.len() + 2
    } else if matches!(failure, Failure::ClosingPrefix) {
        // One primary, 1,000 skipped handles, then the refused live observer.
        1002
    } else {
        1
    };
    assert_eq!(
        remaining,
        allowance.saturating_sub(charge),
        "failed routing escaped the worker allowance after {} observer deliveries",
        packets.len()
    );
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), before + 1);
    assert!(connection.registration.is_none());
    assert!(
        !connection
            .room_fanout
            .as_ref()
            .is_some_and(|job| job.active())
    );
    assert_eq!(
        relay.metrics.queued_bytes.load(Relaxed),
        packets
            .iter()
            .chain(&held)
            .map(|p| p.frame.len())
            .sum::<usize>()
    );
    drop(packets);
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(observers);
    drop(fillers);
    drop(source);
    drop(connection);
    drop(socket);
    drop(client);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
}
