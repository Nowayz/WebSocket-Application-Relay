use super::*;

#[derive(Default)]
struct Recorded(Mutex<Vec<Frame>>);
impl PeerOutput for Recorded {
    fn send(&self, submission: Submission<'_>) -> bool {
        self.0.lock().push(submission.frame.clone());
        true
    }
    fn close(&self, _: Disconnect) {}
}

fn peer(relay: &Arc<Relay>, channel: &[u8], flags: u8) -> (Registration, Arc<Recorded>) {
    let output = Arc::new(Recorded::default());
    let registered = relay
        .subscribe_with_output(channel, output.clone())
        .unwrap();
    relay
        .receive(
            &registered.peer,
            protocol::request(0, &protocol::text("messages")),
        )
        .unwrap();
    relay
        .receive(
            &registered.peer,
            protocol::request(1, &protocol::uint(u64::from(flags))),
        )
        .unwrap();
    output.0.lock().clear();
    (registered, output)
}

fn relay() -> Arc<Relay> {
    Relay::new(Config {
        auth_password: Some("messages".into()),
        ..Config::default()
    })
    .unwrap()
}

fn data(output: &Recorded) -> Vec<Frame> {
    output
        .0
        .lock()
        .iter()
        .filter(|f| {
            protocol::parse(&f.to_vec()).is_ok_and(|(sender, _)| sender != 0 && sender != BROADCAST)
        })
        .cloned()
        .collect()
}

#[test]
fn observer_pressure_drops_output_without_failing_the_source() {
    for first_turn in [0, 1] {
        let relay = Relay::new(Config {
            auth_password: Some("messages".into()),
            max_message_bytes: 1024,
            queue_bytes: 2048,
            total_queue_bytes: 16384,
            ..Config::default()
        })
        .unwrap();
        let mut source = relay.subscribe(b"count-room").unwrap();
        drop(source.outgoing.try_recv().unwrap());
        let mut reader = relay.subscribe(b"count-room").unwrap();
        drop(reader.outgoing.try_recv().unwrap());
        let mut observers = Vec::new();
        for _ in 0..FANOUT_QUANTUM + 44 {
            let mut observer = relay.subscribe(GLOBAL_CHANNEL).unwrap();
            drop(observer.outgoing.try_recv().unwrap());
            for request in [
                protocol::request(0, &protocol::text("messages")),
                protocol::request(1, b"\x01"),
            ] {
                relay.receive(&observer.registration.peer, request).unwrap();
                drop(observer.outgoing.try_recv().unwrap());
            }
            observers.push(observer);
        }
        let value = protocol::byte_string(&[0x35; 256]);
        let input = protocol::envelope(BROADCAST, &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        let mut pending = None;
        relay
            .receive_batch()
            .receive_bounded(
                &source.registration.peer,
                input,
                0,
                &mut pending,
                first_turn,
            )
            .unwrap();
        pending
            .as_mut()
            .unwrap()
            .resume_routing(FANOUT_QUANTUM)
            .unwrap();
        pending
            .as_mut()
            .unwrap()
            .resume_routing(usize::MAX)
            .unwrap();
        let mut packets = vec![reader.outgoing.try_recv().unwrap()];
        let mut observer_count = 0;
        for observer in &mut observers {
            if let Ok(packet) = observer.outgoing.try_recv() {
                observer_count += 1;
                packets.push(packet);
            }
        }
        // Retained observer output exhausts the shared budget part way through.
        // Later observers miss this message; nobody is closed for it.
        assert!(observer_count > 0 && observer_count < observers.len());
        assert!(relay.metrics.global_pressure.load(Relaxed) >= 1);
        assert!(!source.registration.peer.closing.load(Relaxed));
        assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 0);
        for packet in &packets {
            assert_eq!(packet.frame.payload().as_ptr(), pointer);
            assert_eq!(
                packet.frame.to_vec(),
                protocol::envelope(source.registration.peer.id.load(Relaxed), &value)
            );
        }
        assert!(!pending.as_ref().unwrap().active());
        assert!(relay.messages.index.lock().jobs.is_empty());
        assert_eq!(
            relay.metrics.queued_bytes.load(Relaxed),
            packets.iter().map(|p| p.frame.len()).sum::<usize>()
        );
        drop(packets);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        relay.begin_shutdown();
    }
}

#[test]
fn partial_observer_publications_flush_only_the_revoking_peer_and_never_replay() {
    for flag in [CHANNEL_MESSAGE, PRIVATE_MESSAGE] {
        let relay = relay();
        let (source, _) = peer(&relay, b"source", 0);
        let (target, target_output) = peer(&relay, b"target", 0);
        let observers: Vec<_> = (0..3).map(|_| peer(&relay, GLOBAL_CHANNEL, flag)).collect();
        let destination = if flag == CHANNEL_MESSAGE {
            BROADCAST
        } else {
            target.peer.id.load(Relaxed)
        };
        let input = protocol::envelope(destination, &protocol::byte_string(&[0x35; 512]));
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        let mut pending = None;
        relay
            .receive_batch()
            .receive_bounded(&source.peer, input, 0, &mut pending, 1)
            .unwrap();
        assert!(pending.as_ref().unwrap().active());
        let allocation = Arc::as_ptr(&pending.as_ref().unwrap().observed.as_ref().unwrap().work);
        assert_eq!(relay.messages.index.lock().jobs.len(), 1);
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        let (observer, output) = &observers[2];
        assert!(data(output).is_empty());
        let other_before = data(&observers[1].1).len();
        relay
            .receive(&observer.peer, protocol::request(1, b"\x00"))
            .unwrap();
        let packets = output.0.lock();
        assert_eq!(packets.len(), 2);
        assert_eq!(
            packets[0].payload().as_ptr(),
            pointer,
            "barrier copied a body"
        );
        assert_eq!(packets[1].to_vec(), protocol::reply(1, b"\x00"));
        drop(packets);
        assert_eq!(
            data(&observers[1].1).len(),
            other_before,
            "barrier advanced another observer"
        );
        relay
            .receive(
                &observer.peer,
                protocol::request(1, &protocol::uint(u64::from(flag))),
            )
            .unwrap();
        pending.as_mut().unwrap().resume(usize::MAX).unwrap();
        assert!(!pending.as_ref().unwrap().active());
        assert!(relay.messages.index.lock().jobs.is_empty());
        for (_, output) in &observers {
            let packets = data(output);
            assert_eq!(packets.len(), 1);
            assert_eq!(packets[0].payload().as_ptr(), pointer);
        }
        assert_eq!(
            data(&target_output).len(),
            usize::from(flag == PRIVATE_MESSAGE)
        );
        let next = protocol::envelope(destination, b"\xf5");
        relay
            .receive_batch()
            .receive_bounded(&source.peer, next, 0, &mut pending, 1)
            .unwrap();
        pending.as_mut().unwrap().resume(usize::MAX).unwrap();
        for (_, output) in &observers {
            assert_eq!(data(output).len(), 2);
            assert_eq!(data(output)[1].payload(), b"\xf5");
        }
        assert_eq!(
            Arc::as_ptr(&pending.as_ref().unwrap().observed.as_ref().unwrap().work),
            allocation,
            "source failed to reuse its publication allocation"
        );
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        relay.begin_shutdown();
    }
}

#[test]
fn private_primary_deduplicates_and_a_global_source_can_observe_its_own_message() {
    let relay = relay();
    let (source, source_output) = peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE);
    let (target, target_output) = peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE);
    let (other, other_output) = peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE);
    let mut pending = None;
    for recipient in [&target.peer, &source.peer] {
        relay
            .receive_batch()
            .receive_bounded(
                &source.peer,
                protocol::envelope(recipient.id.load(Relaxed), b"\xf5"),
                0,
                &mut pending,
                1,
            )
            .unwrap();
        pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    }
    assert_eq!(data(&source_output).len(), 2);
    assert_eq!(data(&target_output).len(), 2);
    assert_eq!(data(&other_output).len(), 2);
    assert!(relay.messages.index.lock().jobs.is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(other);
}

#[test]
fn a_barrier_orders_both_message_classes_without_suppressing_an_unflushed_job() {
    let relay = relay();
    let (channel_source, _) = peer(&relay, b"channel", 0);
    let (private_source, _) = peer(&relay, b"private", 0);
    let (target, _) = peer(&relay, b"target", 0);
    let (observer, output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE | PRIVATE_MESSAGE);
    let mut channel_job = None;
    let mut private_job = None;
    relay
        .receive_batch()
        .receive_bounded(
            &channel_source.peer,
            protocol::envelope(BROADCAST, b"\xf4"),
            0,
            &mut channel_job,
            0,
        )
        .unwrap();
    relay
        .receive_batch()
        .receive_bounded(
            &private_source.peer,
            protocol::envelope(target.peer.id.load(Relaxed), b"\xf5"),
            0,
            &mut private_job,
            0,
        )
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(1, &protocol::uint(u64::from(PRIVATE_MESSAGE))),
        )
        .unwrap();
    let packets = output.0.lock();
    assert_eq!(packets.len(), 3);
    assert_eq!(packets[0].payload(), b"\xf4");
    assert_eq!(packets[1].payload(), b"\xf5");
    assert_eq!(
        packets[2].to_vec(),
        protocol::reply(1, &protocol::uint(u64::from(PRIVATE_MESSAGE)))
    );
    drop(packets);
    private_job.as_mut().unwrap().resume(usize::MAX).unwrap();
    channel_job.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert_eq!(data(&output).len(), 2);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
}

#[test]
fn observer_views_invalidate_by_class_and_pending_eligibility_excludes_late_subscribers() {
    let relay = relay();
    let (source, _) = peer(&relay, b"source", 0);
    let (observer, output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE);
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    let view = relay.message_snapshot_cached(0).unwrap();
    let (late, late_output) = peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE);
    assert!(Arc::ptr_eq(
        &view,
        &relay.message_snapshot_cached(0).unwrap()
    ));
    relay
        .receive(
            &late.peer,
            protocol::request(1, &protocol::uint(u64::from(CHANNEL_MESSAGE))),
        )
        .unwrap();
    assert!(relay.message_snapshot_cached(0).is_none());
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert_eq!(data(&output).len(), 1);
    assert!(data(&late_output).is_empty());
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf4"),
            0,
            &mut pending,
            1,
        )
        .unwrap();
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert_eq!(data(&late_output).len(), 1);
    relay.begin_shutdown();
    assert!((0..2).all(|index| relay.message_snapshot_cached(index).is_none()));
    drop(observer);
}

#[test]
fn observer_cancellation_budget_refusal_and_sequence_exhaustion_leave_no_staging() {
    let relay = relay();
    let (source, _) = peer(&relay, b"source", 0);
    let (observer, output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE);
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    pending.as_mut().unwrap().clear();
    assert!(relay.messages.index.lock().jobs.is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert!(data(&output).is_empty());
    relay.messages.index.lock().sequence = u64::MAX - 1;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    for _ in 0..2 {
        relay
            .receive(&observer.peer, protocol::request(1, b"\x00"))
            .unwrap();
        relay
            .receive(
                &observer.peer,
                protocol::request(1, &protocol::uint(u64::from(CHANNEL_MESSAGE))),
            )
            .unwrap();
    }
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert_eq!(
        data(&output).len(),
        1,
        "maximum sequence replayed at a second barrier"
    );
    // Exhausted publication numbering delivers immediately instead.
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    assert_eq!(data(&output).len(), 2);
    assert!(relay.messages.index.lock().jobs.is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(observer);

    let constrained = Relay::new(Config {
        auth_password: Some("messages".into()),
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: 4096,
        ..Config::default()
    })
    .unwrap();
    let (source, _) = peer(&constrained, b"source", 0);
    let observers: Vec<_> = (0..512)
        .map(|_| peer(&constrained, GLOBAL_CHANNEL, CHANNEL_MESSAGE))
        .collect();
    let mut pending = None;
    // An unfunded observer view is visited immediately instead of staged.
    constrained
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, &protocol::byte_string(&[0x22; 512])),
            0,
            &mut pending,
            1,
        )
        .unwrap();
    assert!(constrained.metrics.global_pressure.load(Relaxed) >= 1);
    assert!(observers.iter().any(|(_, output)| !data(output).is_empty()));
    assert!(!source.peer.closing.load(Relaxed));
    assert!(constrained.messages.index.lock().jobs.is_empty());
    assert_eq!(constrained.metrics.queued_bytes.load(Relaxed), 0);
    constrained.begin_shutdown();
    drop(observers);
}

#[test]
fn private_primary_delivery_precedes_an_unfunded_observer_snapshot() {
    let relay = Relay::new(Config {
        auth_password: Some("messages".into()),
        max_message_bytes: 1024,
        queue_bytes: 2048,
        total_queue_bytes: 4096,
        ..Config::default()
    })
    .unwrap();
    let (source, _) = peer(&relay, b"source", 0);
    let (target, output) = peer(&relay, b"target", 0);
    let observers: Vec<_> = (0..512)
        .map(|_| peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE))
        .collect();
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(target.peer.id.load(Relaxed), b"\xf5"),
            0,
            &mut pending,
            1,
        )
        .unwrap();
    assert_eq!(
        data(&output).len(),
        1,
        "observer staging prevented the primary private delivery"
    );
    // The unfunded view is visited immediately; some observers still miss the
    // message under shared pressure, and the source stays open.
    assert!(observers.iter().any(|(_, output)| !data(output).is_empty()));
    assert!(relay.metrics.global_pressure.load(Relaxed) >= 1);
    assert!(!source.peer.closing.load(Relaxed));
    assert!(relay.messages.index.lock().jobs.is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
}

#[test]
fn a_combined_room_and_observer_publication_reserves_its_body_once() {
    let relay = Relay::new(Config {
        auth_password: Some("messages".into()),
        max_message_bytes: 1024,
        queue_bytes: 1034,
        total_queue_bytes: 65536,
        ..Config::default()
    })
    .unwrap();
    let (source, _) = peer(&relay, b"source", 0);
    let members: Vec<_> = (0..3).map(|_| peer(&relay, b"source", 0)).collect();
    let observers: Vec<_> = (0..32)
        .map(|_| peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE))
        .collect();
    let value = protocol::byte_string(&[0x56; 900]);
    let expected_bytes = protocol::envelope(source.peer.id.load(Relaxed), &value).len();
    let input = protocol::envelope(BROADCAST, &value);
    let value_pointer = protocol::parse(&input).unwrap().1.as_ptr();
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(&source.peer, input, 0, &mut pending, 1)
        .unwrap();
    assert_eq!(source.peer.budget.used.load(Relaxed), expected_bytes);
    let before: Vec<_> = members
        .iter()
        .map(|(_, output)| data(output).len())
        .collect();
    let (observer, output) = observers.last().unwrap();
    relay
        .receive(&observer.peer, protocol::request(1, b"\x00"))
        .unwrap();
    assert_eq!(data(output).len(), 1);
    assert_eq!(source.peer.budget.used.load(Relaxed), expected_bytes);
    assert_eq!(
        members
            .iter()
            .map(|(_, output)| data(output).len())
            .collect::<Vec<_>>(),
        before,
        "observer barrier advanced room recipients"
    );
    relay
        .receive(&observer.peer, protocol::request(1, b"\x01"))
        .unwrap();
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    for (_, output) in members.iter().chain(observers.iter()) {
        let packets = data(output);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].payload().as_ptr(), value_pointer);
    }
    assert_eq!(source.peer.budget.used.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
}

#[test]
fn captured_observer_eligibility_survives_id_takeover_and_stale_registration_removal() {
    let relay = relay();
    let (source, _) = peer(&relay, b"source", 0);
    let (old, old_output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE);
    let (keeper, keeper_output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE);
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    let (replacement, replacement_output) = peer(&relay, GLOBAL_CHANNEL, 0);
    let id = old.peer.id.load(Relaxed);
    relay
        .receive(&replacement.peer, protocol::request(3, &protocol::uint(id)))
        .unwrap();
    relay
        .receive(&replacement.peer, protocol::request(1, b"\x01"))
        .unwrap();
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert!(data(&old_output).is_empty());
    assert!(data(&replacement_output).is_empty());
    assert_eq!(data(&keeper_output).len(), 1);
    drop(old);
    assert!(Arc::ptr_eq(&relay.user(id).unwrap(), &replacement.peer));
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf4"),
            0,
            &mut pending,
            1,
        )
        .unwrap();
    pending.as_mut().unwrap().resume(usize::MAX).unwrap();
    assert_eq!(data(&replacement_output).len(), 1);
    assert_eq!(data(&keeper_output).len(), 2);
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(id, b"\x07"),
            0,
            &mut pending,
            1,
        )
        .unwrap();
    assert_eq!(data(&replacement_output).len(), 2);
    assert!(data(&old_output).is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
    drop(keeper);
}

#[test]
fn alternating_view_cache_distinguishes_equal_sized_generations_and_cached_absence() {
    let relay = relay();
    let sources: Vec<_> = (0..6)
        .map(|i| peer(&relay, format!("s{i}").as_bytes(), 0))
        .collect();
    let (target, _) = peer(&relay, b"target", 0);
    let (keeper, keeper_output) = peer(&relay, GLOBAL_CHANNEL, 3);
    let (observer, output) = peer(&relay, GLOBAL_CHANNEL, 0);
    let mut jobs: Vec<Option<Box<RoomFanout>>> = (0..6).map(|_| None).collect();
    let mut views = Vec::new();
    for index in 0..6 {
        if index == 2 {
            relay
                .receive(&keeper.peer, protocol::request(1, b"\x00"))
                .unwrap();
            relay
                .receive(&observer.peer, protocol::request(1, b"\x03"))
                .unwrap();
            output.0.lock().clear();
        }
        let destination = if index % 2 == 0 {
            BROADCAST
        } else {
            target.peer.id.load(Relaxed)
        };
        relay
            .receive_batch()
            .receive_bounded(
                &sources[index].0.peer,
                protocol::envelope(destination, &protocol::uint(index as u64)),
                0,
                &mut jobs[index],
                0,
            )
            .unwrap();
        views.push(Arc::downgrade(
            &relay.message_snapshot_cached(index % 2).unwrap(),
        ));
    }
    relay
        .receive(&observer.peer, protocol::request(1, b"\x00"))
        .unwrap();
    let packets = data(&output);
    assert_eq!(packets.len(), 4);
    for (index, packet) in packets.iter().enumerate() {
        assert_eq!(
            packet.to_vec(),
            protocol::envelope(
                sources[index + 2].0.peer.id.load(Relaxed),
                &protocol::uint(index as u64 + 2)
            )
        );
    }
    relay
        .receive(&observer.peer, protocol::request(1, b"\x03"))
        .unwrap();
    for job in &mut jobs {
        job.as_mut().unwrap().resume(usize::MAX).unwrap();
    }
    assert_eq!(
        data(&output).len(),
        4,
        "alternating cached generations replayed a publication"
    );
    assert_eq!(data(&keeper_output).len(), 2);
    assert!(
        views.iter().all(|view| view.upgrade().is_none()),
        "lookup cache pinned an old view"
    );
    assert!(relay.messages.index.lock().jobs.is_empty());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    relay.begin_shutdown();
}

#[test]
fn a_progressing_observer_job_releases_membership_and_serializes_a_concurrent_barrier() {
    struct Gated {
        recorded: Recorded,
        pause: AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        proceed: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl PeerOutput for Gated {
        fn send(&self, packet: Submission<'_>) -> bool {
            if protocol::parse(&packet.frame.to_vec())
                .is_ok_and(|(id, _)| id != 0 && id != BROADCAST)
                && self.pause.swap(false, Relaxed)
            {
                self.entered.send(()).unwrap();
                self.proceed
                    .lock()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            self.recorded.send(packet)
        }
        fn close(&self, _: Disconnect) {}
    }
    let relay = relay();
    let (source, _) = peer(&relay, b"source", 0);
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (proceed_tx, proceed_rx) = std::sync::mpsc::sync_channel(1);
    let gated = Arc::new(Gated {
        recorded: Recorded::default(),
        pause: AtomicBool::new(true),
        entered: entered_tx,
        proceed: Mutex::new(proceed_rx),
    });
    let first = relay
        .subscribe_with_output(GLOBAL_CHANNEL, gated.clone())
        .unwrap();
    relay
        .receive(
            &first.peer,
            protocol::request(0, &protocol::text("messages")),
        )
        .unwrap();
    relay
        .receive(
            &first.peer,
            protocol::request(1, &protocol::uint(u64::from(CHANNEL_MESSAGE))),
        )
        .unwrap();
    let (last, last_output) = peer(&relay, GLOBAL_CHANNEL, CHANNEL_MESSAGE);
    let mut pending = None;
    relay
        .receive_batch()
        .receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0,
        )
        .unwrap();
    let mut work = pending.unwrap();
    let producer = std::thread::spawn(move || {
        work.resume(1).unwrap();
        work
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert!(
        relay.registry_unlocked(),
        "output submission retained a membership lock"
    );
    let barrier_relay = relay.clone();
    let observer = last.peer.clone();
    let barrier = std::thread::spawn(move || {
        barrier_relay
            .receive(&observer, protocol::request(1, b"\x00"))
            .unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !relay.listeners_locked() {
        assert!(
            std::time::Instant::now() < deadline,
            "barrier did not acquire membership"
        );
        std::thread::yield_now();
    }
    proceed_tx.send(()).unwrap();
    let mut work = producer.join().unwrap();
    barrier.join().unwrap();
    assert_eq!(data(&last_output).len(), 1);
    work.resume(usize::MAX).unwrap();
    assert_eq!(data(&last_output).len(), 1);
    assert_eq!(data(&gated.recorded).len(), 1);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert!(relay.messages.index.lock().jobs.is_empty());
    relay.begin_shutdown();
}

// Isolate registry/barrier work from socket scheduling. Run separately from
// ordinary tests and confirm any retained change with the native event screen.
#[test]
#[ignore = "manual release diagnostic; allocates a declared observer population"]
fn observer_barrier_event_screen() {
    struct Sink(AtomicUsize);
    impl PeerOutput for Sink {
        fn send(&self, _: Submission<'_>) -> bool {
            self.0.fetch_add(1, Relaxed);
            true
        }
        fn close(&self, _: Disconnect) {}
    }
    let setting = |name: &str, fallback| {
        std::env::var(name).map_or(fallback, |value| {
            value.parse::<usize>().expect("positive diagnostic setting")
        })
    };
    let audience = setting("BENCH_OBSERVERS", 100_000);
    let publishers = setting("BENCH_PUBLISHERS", 64);
    let rounds = setting("BENCH_ROUNDS", 12);
    assert!(audience > 0 && publishers > 0 && rounds > 0);
    let relay = Relay::new(Config {
        auth_password: Some("messages".into()),
        max_connections: audience + publishers + 1,
        max_channel_members: audience,
        ..Config::default()
    })
    .unwrap();
    let sink = Arc::new(Sink(AtomicUsize::new(0)));
    let sources: Vec<_> = (0..publishers)
        .map(|i| {
            relay
                .subscribe_with_output(format!("p{i:08x}").as_bytes(), sink.clone())
                .unwrap()
        })
        .collect();
    let target = relay
        .subscribe_with_output(b"target", sink.clone())
        .unwrap();
    let observers: Vec<_> = (0..audience)
        .map(|_| {
            let peer = relay
                .subscribe_with_output(GLOBAL_CHANNEL, sink.clone())
                .unwrap();
            relay
                .receive(
                    &peer.peer,
                    protocol::request(0, &protocol::text("messages")),
                )
                .unwrap();
            relay
                .receive(&peer.peer, protocol::request(1, b"\x03"))
                .unwrap();
            peer
        })
        .collect();
    let observer = &observers.last().unwrap().peer;
    let mut jobs: Vec<Option<Box<RoomFanout>>> = (0..publishers).map(|_| None).collect();
    let mut samples = Vec::with_capacity(rounds);
    for round in 0..rounds {
        let started = std::time::Instant::now();
        for (index, (source, job)) in sources.iter().zip(&mut jobs).enumerate() {
            let destination = if index % 2 == 0 {
                BROADCAST
            } else {
                target.peer.id.load(Relaxed)
            };
            relay
                .receive_batch()
                .receive_bounded(
                    &source.peer,
                    protocol::envelope(destination, &protocol::uint(round as u64)),
                    0,
                    job,
                    0,
                )
                .unwrap();
        }
        assert_eq!(relay.messages.index.lock().jobs.len(), publishers);
        let prepared = started.elapsed();
        sink.0.store(0, Relaxed);
        let barrier = std::time::Instant::now();
        relay
            .receive(observer, protocol::request(1, b"\x00"))
            .unwrap();
        let revoked = barrier.elapsed();
        assert_eq!(
            sink.0.load(Relaxed),
            publishers + 1,
            "barrier delivered outside the requesting observer"
        );
        for job in &mut jobs {
            job.as_mut().unwrap().clear();
        }
        relay
            .receive(observer, protocol::request(1, b"\x03"))
            .unwrap();
        assert!(relay.messages.index.lock().jobs.is_empty());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        samples.push(serde_json::json!({"prepare_ns":prepared.as_nanos(),"revoke_ns":revoked.as_nanos(),"whole_ns":started.elapsed().as_nanos()}));
    }
    println!(
        "observer_barrier_event_screen={}",
        serde_json::json!({"audience":audience,"publishers":publishers,"rounds":rounds,"samples":samples,"scope":"Release in-process routing/control/cancellation diagnostic; no native socket I/O"})
    );
    relay.begin_shutdown();
}
