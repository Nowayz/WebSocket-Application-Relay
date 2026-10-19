use super::*;

#[derive(Default)]
struct Recording(parking_lot::Mutex<Vec<Frame>>);
impl PeerOutput for Recording {
    fn send(&self, packet: Submission<'_>) -> bool {
        if protocol::parse(&packet.frame.to_vec()).is_ok_and(|(id, _)| id != 0 && id != BROADCAST) {
            self.0.lock().push(packet.frame.clone());
        }
        true
    }
    fn close(&self, _: Disconnect) {}
}

#[test]
fn a_room_continuation_owns_its_snapshot_borrows_the_body_and_releases_its_charge() {
    let relay = Relay::new(Config {
        ping_interval_secs: 0,
        ..Config::default()
    })
    .unwrap();
    let source = relay
        .subscribe_with_output(b"continuation", Arc::new(Recording::default()))
        .unwrap();
    let mut targets = Vec::new();
    let mut outputs = Vec::new();
    for _ in 0..300 {
        let output = Arc::new(Recording::default());
        targets.push(Some(
            relay
                .subscribe_with_output(b"continuation", output.clone())
                .unwrap(),
        ));
        outputs.push(output);
    }
    let input = protocol::envelope(BROADCAST, &protocol::byte_string(&[0x5a; 1024]));
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    let mut slot = None;
    let mut batch = relay.receive_batch();
    assert_eq!(
        batch
            .receive_bounded(&source.peer, input, 0, &mut slot, 256)
            .unwrap(),
        256
    );
    let work = slot.as_mut().unwrap();
    let allocation = &**work as *const RoomFanout;
    assert!(work.active());
    assert!(work.deadline().is_some());
    let charge = work.packet.as_ref().unwrap().frame.len();
    assert_eq!(
        relay.metrics.queued_bytes.load(Relaxed),
        charge + work.snapshot_bytes
    );
    assert_eq!(source.peer.budget.used.load(Relaxed), charge);
    assert_eq!(
        work.packet.as_ref().unwrap().frame.payload().as_ptr(),
        pointer
    );
    let unvisited = outputs.iter().position(|o| o.0.lock().is_empty()).unwrap();
    // Leave and join between chunks. Retained IDs cannot redirect a captured
    // recipient, and a late join cannot receive the unfinished older frame.
    drop(targets[unvisited].take());
    let late = Arc::new(Recording::default());
    let late_peer = relay
        .subscribe_with_output(b"continuation", late.clone())
        .unwrap();
    work.resume(256).unwrap();
    assert!(!work.active());
    assert!(work.deadline().is_none());
    assert!(work.recipients.is_none());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(source.peer.budget.used.load(Relaxed), 0);
    assert!(late.0.lock().is_empty());
    for (index, output) in outputs.iter().enumerate() {
        let sent = output.0.lock();
        assert_eq!(sent.len(), usize::from(index != unvisited));
        if let Some(frame) = sent.first() {
            assert_eq!(frame.payload().as_ptr(), pointer);
        }
    }
    let input = protocol::envelope(BROADCAST, &[0xf5]);
    batch
        .receive_bounded(&source.peer, input, 0, &mut slot, 256)
        .unwrap();
    let work = slot.as_mut().unwrap();
    assert_eq!(
        &**work as *const RoomFanout, allocation,
        "publisher reuses its only continuation allocation"
    );
    assert!(work.active());
    work.clear();
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(source.peer.budget.used.load(Relaxed), 0);
    drop(late_peer);
    relay.begin_shutdown();
    drop(targets);
    drop(source);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn continuation_metadata_cannot_escape_the_global_queue_budget() {
    let relay = Relay::new(Config {
        max_message_bytes: 65536,
        queue_bytes: 65546,
        total_queue_bytes: 65546,
        ping_interval_secs: 0,
        ..Config::default()
    })
    .unwrap();
    let source = relay
        .subscribe_with_output(b"metadata", Arc::new(Recording::default()))
        .unwrap();
    let mut targets = Vec::new();
    for _ in 0..1024 {
        targets.push(
            relay
                .subscribe_with_output(b"metadata", Arc::new(Recording::default()))
                .unwrap(),
        );
    }
    let mut filler = relay.subscribe(b"fill").unwrap();
    drop(filler.outgoing.try_recv().unwrap());
    let held = Frame::envelope(
        source.peer.id.load(Relaxed),
        protocol::byte_string(&vec![0x5a; 60000]),
    );
    filler.registration.peer.enqueue_frame(&held).unwrap();
    let charged = relay.metrics.queued_bytes.load(Relaxed);
    assert_eq!(charged, held.len());
    let mut slot = None;
    let mut batch = relay.receive_batch();
    assert_eq!(
        batch.receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, &[0xf5]),
            0,
            &mut slot,
            256
        ),
        Err(Disconnect::OVERLOADED)
    );
    let work = slot.as_ref().unwrap();
    assert!(!work.active());
    assert_eq!(work.snapshot_bytes, 0);
    assert_eq!(source.peer.budget.used.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), charged);
    assert_eq!(relay.metrics.global_pressure.load(Relaxed), 1);
    drop(filler.outgoing.try_recv().unwrap());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(targets);
    drop(filler);
    drop(source);
    assert_eq!(relay.counts(), (0, 1));
}

struct NativeRetirement;
impl PeerOutput for NativeRetirement {
    fn send(&self, _: Submission<'_>) -> bool {
        true
    }
    fn close(&self, _: Disconnect) {}
    fn supports_retirement(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct Departures(parking_lot::Mutex<Vec<Frame>>);
impl PeerOutput for Departures {
    fn send(&self, packet: Submission<'_>) -> bool {
        if protocol::parse(&packet.frame.to_vec()).is_ok_and(|(id, _)| id == BROADCAST) {
            self.0.lock().push(packet.frame.clone());
        }
        true
    }
    fn close(&self, _: Disconnect) {}
}

#[test]
fn retirement_sequence_survives_the_small_room_threshold() {
    check_retirement_sequence(false);
}

#[test]
fn retirement_budget_refusal_finishes_older_work_before_synchronous_fallback() {
    check_retirement_sequence(true);
}

fn check_retirement_sequence(pressure: bool) {
    let relay = Relay::new(Config {
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: if pressure { 12000 } else { 256 * 1024 * 1024 },
        ping_interval_secs: 0,
        ..Config::default()
    })
    .unwrap();
    let first = relay
        .subscribe_with_output(b"sequence", Arc::new(NativeRetirement))
        .unwrap();
    let second = relay
        .subscribe_with_output(b"sequence", Arc::new(NativeRetirement))
        .unwrap();
    let first_id = first.peer.id.load(Relaxed);
    let second_id = second.peer.id.load(Relaxed);
    let mut targets = Vec::new();
    let mut outputs = Vec::new();
    for _ in 0..FANOUT_QUANTUM {
        let output = Arc::new(Departures::default());
        targets.push(
            relay
                .subscribe_with_output(b"sequence", output.clone())
                .unwrap(),
        );
        outputs.push(output);
    }
    drop(first);
    assert!(relay.has_retirements());
    // The second departure crosses back to the synchronous-size threshold.
    // It must still follow the unfinished first job, including on budget refusal.
    drop(second);
    if pressure {
        assert!(!relay.has_retirements());
        assert_eq!(relay.metrics.global_pressure.load(Relaxed), 1);
    } else {
        relay.progress_retirements();
        relay.progress_retirements();
        assert!(relay.has_retirements());
        assert!(outputs.iter().all(|o| o.0.lock().len() == 1));
        relay.progress_retirements();
        assert!(!relay.has_retirements());
    }
    for output in outputs {
        let packets = output.0.lock();
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[0].to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(first_id))
        );
        assert_eq!(
            packets[1].to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(second_id))
        );
    }
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(targets);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn a_pending_departure_cannot_notify_a_recreated_room() {
    let relay = Relay::new(Config::default()).unwrap();
    let source = relay
        .subscribe_with_output(b"recreated", Arc::new(NativeRetirement))
        .unwrap();
    let targets: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| {
            relay
                .subscribe_with_output(b"recreated", Arc::new(NativeRetirement))
                .unwrap()
        })
        .collect();
    drop(source);
    assert!(relay.has_retirements());
    drop(targets);
    assert_eq!(relay.counts(), (0, 1));
    assert!(relay.has_retirements());
    let output = Arc::new(Departures::default());
    let replacement = relay
        .subscribe_with_output(b"recreated", output.clone())
        .unwrap();
    // One two-turn initial job plus at most 1,024 single-turn departures.
    // A broken cursor/retirement flag must fail this check rather than spin.
    for _ in 0..FANOUT_QUANTUM + 2 {
        if !relay.has_retirements() {
            break;
        }
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert!(output.0.lock().is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(replacement);
    assert_eq!(relay.counts(), (0, 1));
}

struct WakeCounter {
    wake: mio::Waker,
    calls: AtomicUsize,
}
impl PeerOutput for WakeCounter {
    fn send(&self, _: Submission<'_>) -> bool {
        true
    }
    fn close(&self, _: Disconnect) {}
    fn supports_retirement(&self) -> bool {
        true
    }
    fn wake_retirement(&self) {
        self.calls.fetch_add(1, Relaxed);
        self.wake.wake().unwrap();
    }
}

#[test]
fn retirement_publications_share_one_wake_until_the_queue_drains() {
    let relay = Relay::new(Config::default()).unwrap();
    let poll = mio::Poll::new().unwrap();
    let output = Arc::new(WakeCounter {
        wake: mio::Waker::new(poll.registry(), mio::Token(1)).unwrap(),
        calls: AtomicUsize::new(0),
    });
    let first = relay
        .subscribe_with_output(b"wake", output.clone())
        .unwrap();
    let second = relay
        .subscribe_with_output(b"wake", output.clone())
        .unwrap();
    let third = relay
        .subscribe_with_output(b"wake", output.clone())
        .unwrap();
    let targets: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| {
            relay
                .subscribe_with_output(b"wake", Arc::new(NativeRetirement))
                .unwrap()
        })
        .collect();
    drop(first);
    assert_eq!(output.calls.load(Relaxed), 1);
    drop(second);
    assert_eq!(
        output.calls.load(Relaxed),
        1,
        "an active queue already has a waking or running owner"
    );
    for _ in 0..4 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    drop(third);
    assert!(relay.has_retirements());
    assert_eq!(
        output.calls.load(Relaxed),
        2,
        "refilling an empty queue must wake again"
    );
    for _ in 0..2 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(targets);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn disconnect_observer_notice_cannot_arrive_after_unsubscribe_acknowledgment() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        ..Config::default()
    })
    .unwrap();
    let source = relay
        .subscribe_with_output(b"observer-ack", Arc::new(NativeRetirement))
        .unwrap();
    let source_id = source.peer.id.load(Relaxed);
    let second = relay
        .subscribe_with_output(b"observer-ack", Arc::new(NativeRetirement))
        .unwrap();
    let second_id = second.peer.id.load(Relaxed);
    let global = relay
        .subscribe_with_output(GLOBAL_CHANNEL, Arc::new(NativeRetirement))
        .unwrap();
    let global_id = global.peer.id.load(Relaxed);
    let output = Arc::new(Departures::default());
    let targets: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| {
            relay
                .subscribe_with_output(b"observer-ack", output.clone())
                .unwrap()
        })
        .collect();
    let mut observer = relay.subscribe(GLOBAL_CHANNEL).unwrap();
    drop(observer.outgoing.try_recv().unwrap());
    relay
        .receive(
            &observer.registration.peer,
            protocol::request(0, &protocol::text("observer-ack")),
        )
        .unwrap();
    drop(observer.outgoing.try_recv().unwrap());
    relay
        .receive(
            &observer.registration.peer,
            protocol::request(1, &protocol::uint(u64::from(DISCONNECT_MESSAGE))),
        )
        .unwrap();
    drop(observer.outgoing.try_recv().unwrap());
    drop(source);
    assert!(relay.has_retirements());
    // Mix a global-room departure between two ordinary-room departures. All
    // three reach the observer, but only two reach ordinary room members.
    drop(global);
    drop(second);
    relay
        .receive(
            &observer.registration.peer,
            protocol::request(1, &protocol::uint(0)),
        )
        .unwrap();
    assert!(
        relay.has_retirements(),
        "ordinary room work must remain deferred"
    );
    assert!(
        output.0.lock().is_empty(),
        "revocation must not pump room output"
    );
    let expected = [source_id, global_id, second_id];
    let ack = protocol::reply(1, &protocol::uint(0));
    let mut acknowledged = false;
    let mut notices = 0;
    while let Ok(packet) = observer.outgoing.try_recv() {
        let bytes = packet.frame.to_vec();
        if bytes == ack {
            assert!(!acknowledged);
            acknowledged = true;
        } else {
            assert_eq!(
                bytes,
                protocol::envelope(BROADCAST, &protocol::uint(expected[notices]))
            );
            notices += 1;
            assert!(
                !acknowledged,
                "a notice must precede the revocation acknowledgment"
            );
        }
    }
    assert!(acknowledged);
    for _ in 0..4 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert!(
        observer.outgoing.try_recv().is_err(),
        "revoked listener received an old departure after its acknowledgment"
    );
    assert_eq!(
        notices, 3,
        "an already-authorized notice must precede revocation, not disappear"
    );
    let packets = output.0.lock();
    assert_eq!(packets.len(), targets.len() * 2);
    for (packets, id) in packets.chunks(targets.len()).zip([source_id, second_id]) {
        assert!(
            packets
                .iter()
                .all(|p| p.to_vec() == protocol::envelope(BROADCAST, &protocol::uint(id)))
        );
    }
    drop(packets);
    relay.begin_shutdown();
    drop(observer);
    drop(targets);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(relay.counts(), (0, 1));
}
