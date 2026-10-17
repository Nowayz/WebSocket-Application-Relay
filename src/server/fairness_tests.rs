use super::*;
use crate::relay::{PeerOutput, Submission};
use std::sync::atomic::AtomicUsize;
use tungstenite::protocol::Role;

#[derive(Default)]
struct Sequence {
    messages: AtomicUsize,
}

impl PeerOutput for Sequence {
    fn send(&self, packet: Submission<'_>) -> bool {
        if !packet.frame.payload().is_empty() {
            let bytes = packet.frame.to_vec();
            let (sender, value) = protocol::parse(&bytes).unwrap();
            if sender != protocol::BROADCAST {
                let next = self.messages.fetch_add(1, Relaxed) + 1;
                assert_eq!(value, [next as u8], "recipient FIFO changed");
            }
        }
        true
    }

    fn close(&self, _: Disconnect) {}
}

#[test]
fn a_large_broadcast_yields_before_the_next_buffered_message_without_splitting_fanout() {
    let relay = Relay::new(crate::Config {
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
            .subscribe_with_output(b"wide", socket.clone())
            .unwrap(),
    );
    let mut targets = Vec::new();
    let mut outputs = Vec::new();
    for _ in 0..300 {
        let output = Arc::new(Sequence::default());
        targets.push(
            relay
                .subscribe_with_output(b"wide", output.clone())
                .unwrap(),
        );
        outputs.push(output);
    }
    let mut input = Vec::new();
    for value in [1, 2] {
        let body = protocol::envelope(protocol::BROADCAST, &protocol::uint(value));
        input.extend_from_slice(&[0x82, 0x80 | body.len() as u8, 0, 0, 0, 0]);
        input.extend_from_slice(&body);
    }
    // Both masked frames are already in decoder storage: no timing or TCP
    // arrival can make the old frame-only quota accidentally pass this check.
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
    let mut scratch = ReadStorage::default();
    assert!(
        connection.drive(&relay, &mut scratch, false) == Drive::Again,
        "recipient work must yield even with more complete input buffered"
    );
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 1);
    assert!(
        outputs.iter().all(|out| out.messages.load(Relaxed) == 1),
        "one complete broadcast must reach every recipient before yielding"
    );
    assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 2);
    assert!(outputs.iter().all(|out| out.messages.load(Relaxed) == 2));
    assert!(connection.drive(&relay, &mut scratch, false) == Drive::Wait);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(targets);
    drop(connection);
    drop(socket);
    drop(client);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
}
