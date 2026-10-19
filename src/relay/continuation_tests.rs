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
fn terminal_removal_discards_cached_membership_and_preserves_captured_views() {
    for capture in [false, true] {
        let relay = Relay::new(Config::default()).unwrap();
        let first = relay
            .subscribe_with_output(b"terminal-view", Arc::new(NativeRetirement))
            .unwrap();
        let second = relay
            .subscribe_with_output(b"terminal-view", Arc::new(NativeRetirement))
            .unwrap();
        let third = relay
            .subscribe_with_output(b"terminal-view", Arc::new(NativeRetirement))
            .unwrap();
        relay
            .receive(&first.peer, protocol::envelope(BROADCAST, &[0xf6]))
            .unwrap();
        let captured = {
            let state = relay.state.read();
            let view = state.channels[b"terminal-view".as_slice()]
                .broadcast
                .get()
                .unwrap();
            assert_eq!(Arc::strong_count(view), 1);
            assert_eq!(Arc::weak_count(view), 0);
            capture.then(|| view.clone())
        };
        let first_key = first.peer.key;
        let first_weak = Arc::downgrade(&first.peer);
        relay.begin_shutdown();
        drop(first);
        assert!(
            relay.state.read().channels[b"terminal-view".as_slice()]
                .broadcast
                .get()
                .is_none(),
            "terminal removal has no cached recipient index to maintain"
        );
        if let Some(view) = &captured {
            assert_eq!(view.len(), 3);
            assert!(view.iter().any(|peer| peer.key == first_key));
            assert!(
                first_weak.upgrade().is_some(),
                "captured views retain their owners"
            );
        }
        drop(captured);
        assert!(first_weak.upgrade().is_none());
        drop(second);
        drop(third);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}

#[test]
fn reversible_drain_keeps_the_live_room_view_consistent() {
    let relay = Relay::new(Config::default()).unwrap();
    let first = relay
        .subscribe_with_output(b"drain-view", Arc::new(NativeRetirement))
        .unwrap();
    let second = relay
        .subscribe_with_output(b"drain-view", Arc::new(NativeRetirement))
        .unwrap();
    relay
        .receive(&first.peer, protocol::envelope(BROADCAST, &[0xf6]))
        .unwrap();
    assert!(relay.set_draining(true));
    drop(second);
    {
        let state = relay.state.read();
        let view = state.channels[b"drain-view".as_slice()]
            .broadcast
            .get()
            .unwrap();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].key, first.peer.key);
    }
    assert!(relay.set_draining(false));
    let third = relay
        .subscribe_with_output(b"drain-view", Arc::new(NativeRetirement))
        .unwrap();
    {
        let state = relay.state.read();
        let view = state.channels[b"drain-view".as_slice()]
            .broadcast
            .get()
            .unwrap();
        assert_eq!(view.len(), 2);
        assert!(view.iter().any(|peer| peer.key == third.peer.key));
    }
    relay.begin_shutdown();
    drop(first);
    drop(third);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn retirement_budget_refusal_finishes_older_work_before_synchronous_fallback() {
    check_retirement_sequence(true);
}

#[test]
fn local_retirement_pressure_keeps_unrelated_room_work_deferred() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        max_message_bytes: 65536,
        queue_bytes: 65546,
        ping_interval_secs: 0,
        ..Config::default()
    })
    .unwrap();
    let (observer, observed) = recorded_disconnect_observer(&relay);
    let large = relay
        .subscribe_with_output(b"pressure-large", Arc::new(NativeRetirement))
        .unwrap();
    let large_id = large.peer.id.load(Relaxed);
    let outputs: Vec<_> = (0..=FANOUT_QUANTUM)
        .map(|_| Arc::new(Departures::default()))
        .collect();
    let targets: Vec<_> = outputs
        .iter()
        .map(|output| {
            relay
                .subscribe_with_output(b"pressure-large", output.clone())
                .unwrap()
        })
        .collect();
    let small = relay
        .subscribe_with_output(b"pressure-small", Arc::new(NativeRetirement))
        .unwrap();
    let small_id = small.peer.id.load(Relaxed);
    let output = Arc::new(Departures::default());
    let target = relay
        .subscribe_with_output(b"pressure-small", output.clone())
        .unwrap();
    // Hold an actual charged envelope, as a partially written native echo does.
    // The source cannot fund another departure body from its local allowance.
    let frame = Frame::envelope(BROADCAST, Bytes::from(vec![0; 65526]));
    assert_eq!(frame.len(), 65536);
    let held = small.peer.charge(&frame).unwrap().unwrap().into_owned();
    assert_eq!(small.peer.budget.used.load(Relaxed), 65536);
    drop(large);
    assert!(relay.has_retirements());
    drop(small);
    assert!(
        outputs.iter().all(|o| o.0.lock().is_empty()),
        "local fallback synchronously visited a disjoint room"
    );
    assert!(relay.has_retirements());
    let expected = protocol::envelope(BROADCAST, &protocol::uint(small_id));
    assert_eq!(output.0.lock()[0].to_vec(), expected);
    assert_eq!(output.0.lock().len(), 1);
    // The global observer overlaps both departures. Its preceding notification
    // must be finished before the newer synchronous notice, while the large
    // room itself remains deferred.
    {
        let packets = observed.0.lock();
        assert_eq!(packets.len(), 2);
        for (packet, id) in packets.iter().zip([large_id, small_id]) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    assert_eq!(relay.metrics.global_pressure.load(Relaxed), 0);
    assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 0);
    drop(held);
    for _ in 0..2 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    for output in &outputs {
        let packets = output.0.lock();
        assert_eq!(packets.len(), 1);
        assert_eq!(
            packets[0].to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(large_id))
        );
    }
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(targets);
    drop(target);
    drop(observer);
    assert_eq!(relay.counts(), (0, 1));
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
    let bystander_output = Arc::new(Departures::default());
    let bystander = relay
        .subscribe_with_output(GLOBAL_CHANNEL, bystander_output.clone())
        .unwrap();
    relay
        .receive(
            &bystander.peer,
            protocol::request(0, &protocol::text("observer-ack")),
        )
        .unwrap();
    relay
        .receive(&bystander.peer, protocol::request(1, &protocol::uint(4)))
        .unwrap();
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
    assert!(
        bystander_output.0.lock().is_empty(),
        "revocation must not pump another global recipient"
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
    relay
        .receive(
            &observer.registration.peer,
            protocol::request(1, &protocol::uint(4)),
        )
        .unwrap();
    assert_eq!(
        observer.outgoing.try_recv().unwrap().frame.to_vec(),
        protocol::reply(1, &protocol::uint(4))
    );
    let fresh = relay
        .subscribe_with_output(b"observer-ack", Arc::new(NativeRetirement))
        .unwrap();
    let fresh_id = fresh.peer.id.load(Relaxed);
    drop(fresh);
    // Three two-turn room jobs and four single-turn global jobs.
    for _ in 0..10 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert_eq!(
        observer.outgoing.try_recv().unwrap().frame.to_vec(),
        protocol::envelope(BROADCAST, &protocol::uint(fresh_id))
    );
    assert!(
        observer.outgoing.try_recv().is_err(),
        "revoked listener received an old departure after its acknowledgment"
    );
    assert_eq!(
        notices, 3,
        "an already-authorized notice must precede revocation, not disappear"
    );
    let packets = output.0.lock();
    assert_eq!(packets.len(), targets.len() * 3);
    for (packets, id) in packets
        .chunks(targets.len())
        .zip([source_id, second_id, fresh_id])
    {
        assert!(
            packets
                .iter()
                .all(|p| p.to_vec() == protocol::envelope(BROADCAST, &protocol::uint(id)))
        );
    }
    drop(packets);
    let bystander_packets = bystander_output.0.lock();
    assert_eq!(bystander_packets.len(), 4);
    for (packet, id) in bystander_packets
        .iter()
        .zip([source_id, global_id, second_id, fresh_id])
    {
        assert_eq!(
            packet.to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(id))
        );
    }
    drop(bystander_packets);
    relay.begin_shutdown();
    drop(observer);
    drop(bystander);
    drop(targets);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(relay.counts(), (0, 1));
}

fn recorded_disconnect_observer(relay: &Arc<Relay>) -> (Registration, Arc<Departures>) {
    let output = Arc::new(Departures::default());
    let observer = relay
        .subscribe_with_output(GLOBAL_CHANNEL, output.clone())
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(0, &protocol::text("observer-ack")),
        )
        .unwrap();
    relay
        .receive(&observer.peer, protocol::request(1, &protocol::uint(4)))
        .unwrap();
    (observer, output)
}

#[test]
fn unchanged_disconnect_jobs_share_views_without_pinning_them_after_drain() {
    fn snapshot(job: &RoomFanout) -> &Arc<RetirementSnapshot> {
        match job.recipients.as_ref().unwrap() {
            FanoutRecipients::Disconnect(snapshot) => snapshot,
            FanoutRecipients::Room(_) => panic!("expected disconnect snapshot"),
        }
    }
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        ..Config::default()
    })
    .unwrap();
    let observers: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    let depart = || {
        let source = relay
            .subscribe_with_output(b"snapshot-source", Arc::new(NativeRetirement))
            .unwrap();
        let id = source.peer.id.load(Relaxed);
        drop(source);
        id
    };
    let first = depart();
    let second = depart();
    let old_view = {
        let pending = relay.retirements.lock();
        assert_eq!(pending.jobs.len(), 2);
        let a = snapshot(&pending.jobs[0]);
        let b = snapshot(&pending.jobs[1]);
        assert!(
            Arc::ptr_eq(a, b),
            "unchanged disconnect membership should be captured once"
        );
        Arc::downgrade(a)
    };
    // Other observation classes do not change disconnect membership.
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(5)),
        )
        .unwrap();
    let third = depart();
    {
        let pending = relay.retirements.lock();
        assert!(std::sync::Weak::ptr_eq(
            &old_view,
            &Arc::downgrade(snapshot(&pending.jobs[2]))
        ));
    }
    // A newly authorized observer gets only subsequent departures. Captured
    // jobs retain their old membership while the next job takes a fresh view.
    let (late, late_output) = recorded_disconnect_observer(&relay);
    let fourth = depart();
    let new_view = {
        let pending = relay.retirements.lock();
        let view = Arc::downgrade(snapshot(&pending.jobs[3]));
        assert!(!std::sync::Weak::ptr_eq(&old_view, &view));
        view
    };
    for _ in 0..2 {
        relay.progress_retirements();
    }
    assert!(
        old_view.upgrade().is_some(),
        "other jobs still own the old view and its credit"
    );
    for _ in 0..4 {
        relay.progress_retirements();
    }
    assert!(
        old_view.upgrade().is_none(),
        "the final old-view job releases its view"
    );
    {
        let pending = relay.retirements.lock();
        assert_eq!(pending.jobs.len(), 1);
        let job = &pending.jobs[0];
        assert_eq!(
            relay.metrics.queued_bytes.load(Relaxed),
            snapshot(job).bytes + job.snapshot_bytes + job.packet.as_ref().unwrap().frame.len(),
            "only the remaining job, its body and its shared view retain credit"
        );
    }
    for _ in 0..2 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert!(
        old_view.upgrade().is_none() && new_view.upgrade().is_none(),
        "the cache must not retain recipient storage or peers after jobs drain"
    );
    for (_, output) in &observers {
        let packets = output.0.lock();
        assert_eq!(packets.len(), 4);
        for (packet, id) in packets.iter().zip([first, second, third, fourth]) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    assert_eq!(late_output.0.lock().len(), 1);
    assert_eq!(
        late_output.0.lock()[0].to_vec(),
        protocol::envelope(BROADCAST, &protocol::uint(fourth))
    );
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(late);
    drop(observers);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn shared_disconnect_views_preserve_the_queue_budget_without_per_job_overcharging() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: 12000,
        ..Config::default()
    })
    .unwrap();
    let observers: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    let mut departed = Vec::new();
    for _ in 0..8 {
        let source = relay
            .subscribe_with_output(b"shared-credit", Arc::new(NativeRetirement))
            .unwrap();
        departed.push(source.peer.id.load(Relaxed));
        drop(source);
    }
    assert_eq!(
        relay.retirements.lock().jobs.len(),
        8,
        "one charged view and eight small jobs fit without synchronous fallback"
    );
    assert!(relay.metrics.queued_bytes.load(Relaxed) <= 12000);
    assert_eq!(relay.metrics.global_pressure.load(Relaxed), 0);
    for _ in 0..16 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    for (_, output) in &observers {
        let packets = output.0.lock();
        assert_eq!(packets.len(), departed.len());
        for (packet, &id) in packets.iter().zip(&departed) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    relay.begin_shutdown();
    drop(observers);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn an_unfunded_disconnect_view_falls_back_without_retaining_unaccounted_storage() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: 4096,
        ..Config::default()
    })
    .unwrap();
    let observers: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    let source = relay
        .subscribe_with_output(b"view-refusal", Arc::new(NativeRetirement))
        .unwrap();
    let id = source.peer.id.load(Relaxed);
    drop(source);
    assert!(!relay.has_retirements());
    assert!(relay.state.read().disconnect_snapshot.upgrade().is_none());
    assert_eq!(relay.metrics.global_pressure.load(Relaxed), 1);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    for (_, output) in &observers {
        let packets = output.0.lock();
        assert_eq!(packets.len(), 1);
        assert_eq!(
            packets[0].to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(id))
        );
    }
    relay.begin_shutdown();
    drop(observers);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn observer_barrier_distinguishes_absence_and_positions_in_captured_generations() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        ..Config::default()
    })
    .unwrap();
    let older: Vec<_> = (0..=FANOUT_QUANTUM)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    let depart = || {
        let source = relay
            .subscribe_with_output(b"gen-source", Arc::new(NativeRetirement))
            .unwrap();
        let id = source.peer.id.load(Relaxed);
        drop(source);
        id
    };
    let first = depart();
    let second = depart();
    relay
        .receive(&older[0].0.peer, protocol::request(1, &protocol::uint(0)))
        .unwrap();
    let (observer, output) = recorded_disconnect_observer(&relay);
    let third = depart();
    // The new observer was absent from the first shared view and is last in
    // the second. Move it to position zero in a third view of the same length;
    // equal lengths do not make distinct captured generations interchangeable.
    relay
        .receive(
            &older[FANOUT_QUANTUM].0.peer,
            protocol::request(1, &protocol::uint(0)),
        )
        .unwrap();
    let (replacement, replaced_output) = recorded_disconnect_observer(&relay);
    assert_eq!(
        relay.state.read().listeners(DISCONNECT_MESSAGE)[0].key,
        observer.peer.key
    );
    let fourth = depart();
    let fifth = depart();
    relay
        .receive(&observer.peer, protocol::request(1, &protocol::uint(0)))
        .unwrap();
    for _ in 0..10 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    for (frames, expected) in [
        (&output, vec![third, fourth, fifth]),
        (&older[0].1, vec![first, second]),
        (&older[FANOUT_QUANTUM].1, vec![first, second, third]),
        (&older[1].1, vec![first, second, third, fourth, fifth]),
        (&replaced_output, vec![fourth, fifth]),
    ] {
        let packets = frames.0.lock();
        assert_eq!(packets.len(), expected.len());
        for (packet, id) in packets.iter().zip(expected) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(older);
    drop(observer);
    drop(replacement);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn independent_observer_barriers_skip_visited_notices_and_survive_an_empty_queue() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        ..Config::default()
    })
    .unwrap();
    let observers: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    let depart = || {
        let source = relay
            .subscribe_with_output(b"barrier-source", Arc::new(NativeRetirement))
            .unwrap();
        let id = source.peer.id.load(Relaxed);
        drop(source);
        id
    };
    let first = depart();
    let second = depart();
    // The first observer has already seen the first notice, but the last has not.
    relay.progress_retirements();
    assert_eq!(observers[0].1.0.lock().len(), 1);
    assert!(observers[FANOUT_QUANTUM].1.0.lock().is_empty());
    for index in [0, FANOUT_QUANTUM] {
        relay
            .receive(
                &observers[index].0.peer,
                protocol::request(1, &protocol::uint(0)),
            )
            .unwrap();
        let packets = observers[index].1.0.lock();
        assert_eq!(
            packets.len(),
            2,
            "each observer must receive each old notice exactly once"
        );
        for (packet, id) in packets.iter().zip([first, second]) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    assert_eq!(
        observers[1].1.0.lock().len(),
        1,
        "barriers must not advance another recipient"
    );
    // Re-enable the first observer before the old jobs finish; the last stays off.
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(4)),
        )
        .unwrap();
    let third = depart();
    for _ in 0..6 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    assert_eq!(observers[0].1.0.lock().len(), 3);
    assert_eq!(observers[FANOUT_QUANTUM].1.0.lock().len(), 2);
    // Publication numbering must continue after an empty queue: the old watermark
    // cannot suppress or duplicate a newly published departure.
    let fourth = depart();
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(0)),
        )
        .unwrap();
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(4)),
        )
        .unwrap();
    for _ in 0..2 {
        relay.progress_retirements();
    }
    assert!(!relay.has_retirements());
    let packets = observers[0].1.0.lock();
    assert_eq!(packets.len(), 4);
    for (packet, id) in packets.iter().zip([first, second, third, fourth]) {
        assert_eq!(
            packet.to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(id))
        );
    }
    drop(packets);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(observers);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn exhausted_retirement_numbering_falls_back_in_order_without_wrapping_watermarks() {
    let relay = Relay::new(Config {
        auth_password: Some("observer-ack".into()),
        ..Config::default()
    })
    .unwrap();
    let observers: Vec<_> = (0..FANOUT_QUANTUM + 1)
        .map(|_| recorded_disconnect_observer(&relay))
        .collect();
    relay.retirements.lock().sequence = u64::MAX - 1;
    let depart = || {
        let source = relay
            .subscribe_with_output(b"barrier-source", Arc::new(NativeRetirement))
            .unwrap();
        let id = source.peer.id.load(Relaxed);
        drop(source);
        id
    };
    let first = depart();
    assert!(relay.has_retirements());
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(0)),
        )
        .unwrap();
    relay
        .receive(
            &observers[0].0.peer,
            protocol::request(1, &protocol::uint(4)),
        )
        .unwrap();
    let second = depart();
    assert!(
        !relay.has_retirements(),
        "exhaustion must finish preceding work before direct delivery"
    );
    assert_eq!(relay.retirements.lock().sequence, u64::MAX);
    for (_, output) in &observers {
        let packets = output.0.lock();
        assert_eq!(packets.len(), 2);
        for (packet, id) in packets.iter().zip([first, second]) {
            assert_eq!(
                packet.to_vec(),
                protocol::envelope(BROADCAST, &protocol::uint(id))
            );
        }
    }
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(observers);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn shutdown_releases_listener_indexes_before_retiring_global_sessions() {
    let relay = Relay::new(Config {
        auth_password: Some("shutdown-indexes".into()),
        ..Config::default()
    })
    .unwrap();
    let authenticate = protocol::request(0, &protocol::text("shutdown-indexes"));
    let listen = protocol::request(1, &protocol::uint(7));
    let mut peers = Vec::new();
    for _ in 0..FANOUT_QUANTUM + 1 {
        let peer = relay
            .subscribe_with_output(GLOBAL_CHANNEL, Arc::new(NativeRetirement))
            .unwrap();
        relay.receive(&peer.peer, authenticate.clone()).unwrap();
        relay.receive(&peer.peer, listen.clone()).unwrap();
        peers.push(peer);
    }
    assert!(
        relay
            .state
            .read()
            .listeners
            .iter()
            .all(|v| v.len() == peers.len())
    );
    let weak = Arc::downgrade(&peers[0].peer);
    relay.begin_shutdown();
    relay.begin_shutdown();
    assert!(
        relay
            .state
            .read()
            .listeners
            .iter()
            .all(|v| v.capacity() == 0)
    );
    // Registrations still own these sessions until their transports retire.
    assert_eq!(relay.counts().0, peers.len());
    drop(peers);
    assert!(weak.upgrade().is_none());
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
