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
