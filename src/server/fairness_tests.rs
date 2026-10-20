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
fn a_large_global_broadcast_yields_within_a_message_and_preserves_source_fifo() {
    check_fanout_quantum(FanoutScope::Global, false);
}

#[test]
fn channel_observer_fanout_yields_within_a_frame() {
    check_fanout_quantum(FanoutScope::ChannelObservers, false);
}

#[test]
fn private_observer_fanout_yields_within_a_frame() {
    check_fanout_quantum(FanoutScope::PrivateObservers, false);
}

#[test]
fn cancelling_an_unfinished_room_multicast_releases_staging_without_more_deliveries() {
    check_fanout_quantum(FanoutScope::Room, true);
}

#[test]
fn cancelling_an_unfinished_global_broadcast_releases_staging() {
    check_fanout_quantum(FanoutScope::Global, true);
}

#[test]
fn cancelling_an_unfinished_observer_publication_releases_staging() {
    check_fanout_quantum(FanoutScope::ChannelObservers, true);
    check_fanout_quantum(FanoutScope::PrivateObservers, true);
}

enum FanoutScope {
    Room,
    Global,
    ChannelObservers,
    PrivateObservers,
}

fn check_fanout_quantum(scope: FanoutScope, cancel: bool) {
    check_fanout_allowance(scope, cancel, FANOUT_QUANTUM, FANOUT_QUANTUM + 44);
}

#[test]
fn a_partly_spent_worker_allowance_yields_after_a_native_turn_without_reading_ahead() {
    for scope in [
        FanoutScope::Room,
        FanoutScope::Global,
        FanoutScope::ChannelObservers,
        FanoutScope::PrivateObservers,
    ] {
        check_fanout_allowance(scope, false, 17, FANOUT_QUANTUM + 44);
    }
}

#[test]
fn a_partly_spent_worker_allowance_does_not_add_staging_to_small_fanouts() {
    for scope in [
        FanoutScope::Room,
        FanoutScope::Global,
        FanoutScope::ChannelObservers,
        FanoutScope::PrivateObservers,
    ] {
        check_fanout_allowance(scope, false, 1, 16);
    }
}

fn check_fanout_allowance(
    scope: FanoutScope,
    cancel: bool,
    initial_allowance: usize,
    count: usize,
) {
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
    // Earlier connections may have consumed most of this worker's allowance.
    let mut routing_left = initial_allowance;
    assert!(
        connection.drive_batched(
            &relay,
            &mut scratch,
            false,
            &mut relay.receive_batch(),
            &mut routing_left,
        ) == Drive::Again,
        "recipient work must yield even with more complete input buffered"
    );
    assert_eq!(routing_left, 0);
    assert_eq!(
        relay.metrics.incoming_messages.load(Relaxed),
        incoming_before + 1
    );
    if count < FANOUT_QUANTUM {
        assert!(
            connection.room_fanout.is_none(),
            "worker accounting changed the native deferral cutoff"
        );
        assert!(outputs.iter().all(|out| out.messages.load(Relaxed) == 1));
        let mut routing_left = initial_allowance;
        assert!(
            connection.drive_batched(
                &relay,
                &mut scratch,
                false,
                &mut relay.receive_batch(),
                &mut routing_left,
            ) == Drive::Again
        );
        assert_eq!(routing_left, 0);
        assert!(outputs.iter().all(|out| out.messages.load(Relaxed) == 2));
        assert!(connection.drive(&relay, &mut scratch, false) == Drive::Wait);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        relay.begin_shutdown();
        drop(targets);
        drop(connection);
        drop(socket);
        drop(client);
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
        return;
    }
    if cancel {
        assert!(connection.room_fanout.as_ref().unwrap().active());
        assert!(connection.deadline(&relay).is_some());
        let delivered: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
        // Production publishes terminal shutdown before retiring connections.
        relay.begin_shutdown();
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
    {
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
        if initial_allowance < FANOUT_QUANTUM {
            // Keep the existing source turn, then stop this worker phase.
            // Finishing this job must not consume its next buffered message.
            let mut routing_left = 64;
            assert!(
                connection.drive_batched(
                    &relay,
                    &mut scratch,
                    false,
                    &mut relay.receive_batch(),
                    &mut routing_left,
                ) == Drive::Again
            );
            assert_eq!(routing_left, 0);
            let after: usize = outputs.iter().map(|out| out.messages.load(Relaxed)).sum();
            assert!((1..=FANOUT_QUANTUM).contains(&(after - visited)));
            assert!(!connection.room_fanout.as_ref().unwrap().active());
            assert_eq!(
                relay.metrics.incoming_messages.load(Relaxed),
                incoming_before + 1,
                "continuation read the next source frame early"
            );
        }
        if initial_allowance == FANOUT_QUANTUM {
            assert!(connection.drive(&relay, &mut scratch, false) == Drive::Again);
        }
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
    {
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
        assert_eq!(worker.input_ready.pop_front(), Some(token));
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

#[test]
fn native_departure_work_yields_and_releases_all_reservations() {
    check_departure_work(false, false);
}

#[test]
fn discarding_native_departure_work_releases_staging_without_delivery() {
    check_departure_work(true, false);
}

#[test]
fn native_disconnect_observer_work_yields_and_releases_all_reservations() {
    check_departure_work(false, true);
}

fn check_departure_work(cancel: bool, observers: bool) {
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
    let source = relay
        .subscribe_with_output(b"depart", socket.clone())
        .unwrap();
    let id = source.peer.id.load(Relaxed);
    let mut targets = Vec::new();
    for _ in 0..FANOUT_QUANTUM + 3 {
        let mut target = relay
            .subscribe(if observers {
                protocol::GLOBAL_CHANNEL
            } else {
                b"depart"
            })
            .unwrap();
        drop(target.outgoing.try_recv().unwrap());
        if observers {
            relay
                .receive(
                    &target.registration.peer,
                    protocol::request(0, &protocol::text("fairness")),
                )
                .unwrap();
            relay
                .receive(
                    &target.registration.peer,
                    protocol::request(1, &protocol::uint(u64::from(protocol::DISCONNECT_MESSAGE))),
                )
                .unwrap();
            while target.outgoing.try_recv().is_ok() {}
        }
        targets.push(target);
    }
    if !observers {
        relay
            .receive(&source.peer, protocol::envelope(protocol::BROADCAST, &[1]))
            .unwrap();
    }
    // Policy closes may already have set closing; departure charging must not
    // silently skip the notifications in that case.
    source.peer.close(Disconnect::OVERLOADED);
    drop(source);
    assert!(relay.has_retirements());
    assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
    if !observers {
        for target in &mut targets {
            let earlier = target.outgoing.try_recv().unwrap();
            assert_eq!(earlier.frame.to_vec(), protocol::envelope(id, &[1]));
        }
    }
    assert!(targets.iter_mut().all(|t| t.outgoing.try_recv().is_err()));
    let mut late = relay.subscribe(b"depart").unwrap();
    drop(late.outgoing.try_recv().unwrap());
    if cancel {
        relay.begin_shutdown();
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert!(targets.iter_mut().all(|t| t.outgoing.try_recv().is_err()));
    } else {
        let expected = protocol::envelope(protocol::BROADCAST, &protocol::uint(id));
        relay.progress_retirements(FANOUT_QUANTUM);
        assert!(relay.has_retirements());
        let mut received = 0;
        for t in &mut targets {
            if let Ok(packet) = t.outgoing.try_recv() {
                assert_eq!(packet.frame.to_vec(), expected);
                received += 1;
            }
        }
        assert_eq!(received, FANOUT_QUANTUM);
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        relay.progress_retirements(FANOUT_QUANTUM);
        assert!(!relay.has_retirements());
        for t in &mut targets {
            if let Ok(packet) = t.outgoing.try_recv() {
                assert_eq!(packet.frame.to_vec(), expected);
                received += 1;
            }
        }
        assert_eq!(received, FANOUT_QUANTUM + 3);
        assert!(
            late.outgoing.try_recv().is_err(),
            "late join must not receive an older departure"
        );
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
    relay.begin_shutdown();
    drop(targets);
    drop(late);
    drop(socket);
    drop(client);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
    assert_eq!(relay.counts(), (0, 1));
}
