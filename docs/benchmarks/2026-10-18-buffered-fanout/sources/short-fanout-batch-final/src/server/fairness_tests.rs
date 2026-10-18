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
            if sender != protocol::BROADCAST && sender != protocol::RELAY {
                let next = self.messages.fetch_add(1, Relaxed) + 1;
                assert_eq!(value, [next as u8], "recipient FIFO changed");
            }
        }
        true
    }

    fn close(&self, _: Disconnect) {}
}

#[test]
fn a_large_room_multicast_yields_within_a_message_and_preserves_source_fifo() {
    check_fanout_quantum(FanoutScope::Room, false);
}

#[test]
fn a_large_global_broadcast_yields_without_splitting_fanout() {
    check_fanout_quantum(FanoutScope::Global, false);
}

#[test]
fn channel_observer_fanout_yields_without_splitting_fanout() {
    check_fanout_quantum(FanoutScope::ChannelObservers, false);
}

#[test]
fn private_observer_fanout_yields_without_splitting_fanout() {
    check_fanout_quantum(FanoutScope::PrivateObservers, false);
}

#[test]
fn cancelling_an_unfinished_room_multicast_releases_staging_without_more_deliveries() {
    check_fanout_quantum(FanoutScope::Room, true);
}

enum FanoutScope {
    Room,
    Global,
    ChannelObservers,
    PrivateObservers,
}

fn check_fanout_quantum(scope: FanoutScope, cancel: bool) {
    let relay = Relay::new(crate::Config {
        ping_interval_secs: 0,
        total_read_buffer_bytes: 0,
        auth_password: Some("fairness".into()),
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
    let source_channel = if matches!(scope, FanoutScope::Global) {
        protocol::GLOBAL_CHANNEL
    } else {
        b"wide"
    };
    connection.registration = Some(
        relay
            .subscribe_with_output(source_channel, socket.clone())
            .unwrap(),
    );
    let mut targets = Vec::new();
    let mut outputs = Vec::new();
    let count = if matches!(scope, FanoutScope::Room) {
        FANOUT_QUANTUM + 44
    } else {
        300
    };
    for _ in 0..count {
        let output = Arc::new(Sequence::default());
        let channel = if matches!(scope, FanoutScope::Room) {
            source_channel
        } else {
            protocol::GLOBAL_CHANNEL
        };
        let target = relay
            .subscribe_with_output(channel, output.clone())
            .unwrap();
        if matches!(
            scope,
            FanoutScope::ChannelObservers | FanoutScope::PrivateObservers
        ) {
            relay
                .receive(
                    &target.peer,
                    protocol::request(0, &protocol::text("fairness")),
                )
                .unwrap();
            let flags = if matches!(scope, FanoutScope::ChannelObservers) {
                protocol::CHANNEL_MESSAGE
            } else {
                protocol::PRIVATE_MESSAGE
            };
            relay
                .receive(
                    &target.peer,
                    protocol::request(1, &protocol::uint(u64::from(flags))),
                )
                .unwrap();
        }
        targets.push(target);
        outputs.push(output);
    }
    let incoming_before = relay.metrics.incoming_messages.load(Relaxed);
    let destination = if matches!(scope, FanoutScope::PrivateObservers) {
        connection
            .registration
            .as_ref()
            .unwrap()
            .peer
            .id
            .load(Relaxed)
    } else {
        protocol::BROADCAST
    };
    let mut input = Vec::new();
    let messages = if matches!(scope, FanoutScope::Room) {
        4
    } else {
        2
    };
    for value in 1..=messages {
        let body = protocol::envelope(destination, &protocol::uint(value));
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
    assert_eq!(
        relay.metrics.incoming_messages.load(Relaxed),
        incoming_before
            + if matches!(scope, FanoutScope::Room) {
                4
            } else {
                1
            }
    );
    if cancel {
        assert!(connection.room_fanout.as_ref().unwrap().active());
        assert!(connection.deadline(&relay).is_some());
        let delivered: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
        connection.start_close(Disconnect::SHUTDOWN, &relay, false, Instant::now());
        assert!(!connection.room_fanout.as_ref().unwrap().active());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(
            outputs
                .iter()
                .map(|out| out.messages.load(Relaxed))
                .sum::<usize>(),
            delivered
        );
        relay.begin_shutdown();
        drop(targets);
        drop(connection);
        drop(socket);
        drop(client);
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
        return;
    }
    if matches!(scope, FanoutScope::Room) {
        let mut visited: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
        assert!((1..=FANOUT_QUANTUM).contains(&visited));
        assert!(
            visited < outputs.len(),
            "large multicast must yield within the frame"
        );
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        while connection.room_fanout.as_ref().is_some_and(|f| f.active()) {
            assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
            let next: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
            assert!(
                next - visited <= FANOUT_QUANTUM,
                "batched work exceeded recipient-frame quantum"
            );
            visited = next;
        }
        assert!(outputs.iter().all(|out| out.messages.load(Relaxed) == 4));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert!(connection.drive(&relay, &mut scratch, false) == Drive::Wait);
        relay.begin_shutdown();
        drop(targets);
        drop(connection);
        drop(socket);
        drop(client);
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
        return;
    }
    assert!(outputs.iter().all(|out| out.messages.load(Relaxed) == 1));
    assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
    assert_eq!(
        relay.metrics.incoming_messages.load(Relaxed),
        incoming_before + 2
    );
    if matches!(scope, FanoutScope::Room) {
        assert!(outputs.iter().any(|out| out.messages.load(Relaxed) == 1));
        assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
    }
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

#[test]
fn buffered_classifier_never_crosses_control_fragment_or_incomplete_input() {
    use std::io::Cursor;
    const PREFIX: &[u8] = b"\x82\x1b\xff\xff\xff\xff\xff\xff\xff\xff";
    let body = protocol::envelope(protocol::BROADCAST, b"\xf5");
    let mask = [0x81, 0x11, 0xfe, 0x55];
    let mut input = vec![0x82, 0x80 | body.len() as u8];
    input.extend_from_slice(&mask);
    input.extend(body.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    let peek = |input: Vec<u8>| {
        let ws = WebSocket::from_partially_read(
            Cursor::new(Vec::<u8>::new()),
            input,
            Role::Server,
            None,
        );
        ws.buffered_binary_prefix(PREFIX)
    };
    assert!(peek(input.clone()));
    for length in 0..input.len() {
        assert!(!peek(input[..length].to_vec()));
    }
    for opcode in [0x02, 0x80, 0x81, 0x88, 0x89, 0x8a, 0xc2, 0xa2, 0x92] {
        let mut changed = input.clone();
        changed[0] = opcode;
        assert!(!peek(changed));
    }
    let mut changed = input.clone();
    changed[6] ^= 1;
    assert!(!peek(changed));
    let mut ws =
        WebSocket::from_partially_read(Cursor::new(Vec::<u8>::new()), input, Role::Server, None);
    assert!(ws.buffered_binary_prefix(PREFIX));
    assert_eq!(ws.read().unwrap(), Message::Binary(body));
    assert!(!ws.buffered_binary_prefix(PREFIX));
}
