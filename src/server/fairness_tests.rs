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
    for value in [1, 2] {
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
        incoming_before + 1
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
        let visited: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
        assert!((1..=FANOUT_QUANTUM).contains(&visited));
        assert!(
            visited < outputs.len(),
            "large multicast must yield within the frame"
        );
        assert!(
            relay.metrics.queued_bytes.load(Relaxed) > 0,
            "unfinished payload must stay charged"
        );
        assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
        assert_eq!(
            relay.metrics.incoming_messages.load(Relaxed),
            incoming_before + 1
        );
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

fn singleton_source() -> (Arc<Relay>, Registration, Arc<Sequence>) {
    let relay = Relay::new(crate::Config {
        auth_password: Some("singleton".into()),
        ..crate::Config::default()
    })
    .unwrap();
    let output = Arc::new(Sequence::default());
    let source = relay
        .subscribe_with_output(b"singleton", output.clone())
        .unwrap();
    (relay, source, output)
}

#[test]
fn singleton_broadcasts_skip_self_and_follow_membership_without_a_continuation() {
    let (relay, source, output) = singleton_source();
    let mut incoming = relay.receive_batch();
    let mut pending = None;
    assert_eq!(
        incoming
            .receive_bounded(
                &source.peer,
                protocol::envelope(protocol::BROADCAST, &[1]),
                4096,
                &mut pending,
                FANOUT_QUANTUM
            )
            .unwrap(),
        1
    );
    assert_eq!(output.messages.load(Relaxed), 0);
    assert!(pending.is_none());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);

    let recipient_output = Arc::new(Sequence::default());
    let recipient = relay
        .subscribe_with_output(b"singleton", recipient_output.clone())
        .unwrap();
    assert_eq!(
        incoming
            .receive_bounded(
                &source.peer,
                protocol::envelope(protocol::BROADCAST, &[1]),
                4096,
                &mut pending,
                FANOUT_QUANTUM
            )
            .unwrap(),
        2
    );
    assert_eq!(recipient_output.messages.load(Relaxed), 1);
    drop(recipient);
    assert_eq!(
        incoming
            .receive_bounded(
                &source.peer,
                protocol::envelope(protocol::BROADCAST, &[2]),
                4096,
                &mut pending,
                FANOUT_QUANTUM
            )
            .unwrap(),
        1
    );
    incoming.publish();
    assert_eq!(recipient_output.messages.load(Relaxed), 1);
    assert_eq!(output.messages.load(Relaxed), 0);
    assert!(pending.is_none());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
}

#[test]
fn singleton_room_still_delivers_to_enabled_channel_observers() {
    let (relay, source, output) = singleton_source();
    let observer_output = Arc::new(Sequence::default());
    let observer = relay
        .subscribe_with_output(protocol::GLOBAL_CHANNEL, observer_output.clone())
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(0, &protocol::text("singleton")),
        )
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(1, &protocol::uint(u64::from(protocol::CHANNEL_MESSAGE))),
        )
        .unwrap();
    let mut incoming = relay.receive_batch();
    let mut pending = None;
    let work = incoming
        .receive_bounded(
            &source.peer,
            protocol::envelope(protocol::BROADCAST, &[1]),
            4096,
            &mut pending,
            FANOUT_QUANTUM,
        )
        .unwrap();
    assert_eq!(
        observer_output.messages.load(Relaxed),
        1,
        "enabled observer missed the singleton broadcast"
    );
    assert_eq!(work, 2);
    relay
        .receive(&observer.peer, protocol::request(1, &protocol::uint(0)))
        .unwrap();
    assert_eq!(
        incoming
            .receive_bounded(
                &source.peer,
                protocol::envelope(protocol::BROADCAST, &[2]),
                4096,
                &mut pending,
                FANOUT_QUANTUM
            )
            .unwrap(),
        1
    );
    assert_eq!(observer_output.messages.load(Relaxed), 1);
    relay
        .receive(
            &observer.peer,
            protocol::request(1, &protocol::uint(u64::from(protocol::CHANNEL_MESSAGE))),
        )
        .unwrap();
    assert_eq!(
        incoming
            .receive_bounded(
                &source.peer,
                protocol::envelope(protocol::BROADCAST, &[2]),
                4096,
                &mut pending,
                FANOUT_QUANTUM
            )
            .unwrap(),
        2
    );
    incoming.publish();
    assert_eq!(observer_output.messages.load(Relaxed), 2);
    assert_eq!(output.messages.load(Relaxed), 0);
    assert!(pending.is_none());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
}

#[test]
fn expired_timer_discovery_yields_and_preserves_the_remaining_deadlines() {
    let relay = Relay::new(crate::Config::default()).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut worker = Worker::new(listener, relay).unwrap();
    let now = Instant::now();
    let due = READY_QUANTUM * 2 + 1;
    for slot in 0..due {
        worker.timers.set(slot, Some(now));
    }
    let future = now + Duration::from_secs(1);
    worker.timers.set(due, Some(future));
    worker.schedule_timers(now);
    let mut remaining = 0;
    while worker.timers.pop_due(now).is_some() {
        remaining += 1;
    }
    assert_eq!(remaining, due - READY_QUANTUM);
    assert_eq!(worker.timers.next(), Some(future));
    assert_eq!(worker.timers.pop_due(future), Some(due));
    assert_eq!(worker.timers.next(), None);
}

#[test]
fn readable_upgrades_cannot_bypass_deadlines_or_shutdown_behind_an_expiry_batch() {
    use std::io::Read;

    for stopping in [false, true] {
        let relay = Relay::new(crate::Config {
            total_read_buffer_bytes: 0,
            ..crate::Config::default()
        })
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut worker = Worker::new(listener, relay.clone()).unwrap();
        relay.metrics.connections.fetch_add(1, Relaxed);
        let token = Token(2);
        let socket = Socket::new(
            mio::net::TcpStream::from_std(stream),
            token,
            worker.registry.clone(),
            worker.signal.clone(),
            ConnectionGuard(relay.metrics.clone(), None),
            &relay,
        )
        .unwrap();
        let mut connection = Connection::new(socket.clone(), token, None, &relay);
        let now = Instant::now();
        let deadline = now - Duration::from_millis(1);
        connection.phase_deadline = deadline;
        let slot = worker.connections.insert(connection);
        worker.tokens.insert(token, slot);
        worker.timers.set(slot, Some(deadline));
        // Earlier entries consume the discovery batch. This live connection's
        // timer remains in the heap when its socket becomes readable.
        for other in 1..=READY_QUANTUM {
            worker.timers.set(other, Some(now - Duration::from_secs(1)));
        }
        client.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
        worker.schedule_timers(now);
        assert_eq!(worker.timers.next(), Some(deadline));
        let mut events = Events::with_capacity(4);
        worker
            .poll
            .poll(&mut events, Some(Duration::from_secs(1)))
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.token() == token && event.is_readable())
        );
        worker.schedule(token); // The normal readable-event scheduling path.
        assert_eq!(worker.ready.pop_front(), Some(token));
        let mut connection = worker.connections.remove(slot);
        assert!(matches!(
            connection.drive(&relay, &mut worker.scratch, stopping),
            Drive::Drop
        ));
        assert!(matches!(connection.stage, Stage::Upgrade(_)));
        assert_eq!(
            relay.metrics.handshake_errors.load(Relaxed),
            u64::from(!stopping)
        );
        drop(connection);
        drop(socket);
        let mut response = [0; 1];
        match client.read(&mut response) {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
            result => panic!("late upgrade produced HTTP output or did not retire: {result:?}"),
        }
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}
