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
    let view = relay.state.read().message_snapshots[0]
        .get()
        .unwrap()
        .clone();
    let (late, late_output) = peer(&relay, GLOBAL_CHANNEL, PRIVATE_MESSAGE);
    assert!(Arc::ptr_eq(
        &view,
        relay.state.read().message_snapshots[0].get().unwrap()
    ));
    relay
        .receive(
            &late.peer,
            protocol::request(1, &protocol::uint(u64::from(CHANNEL_MESSAGE))),
        )
        .unwrap();
    assert!(relay.state.read().message_snapshots[0].get().is_none());
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
    assert!(
        relay
            .state
            .read()
            .message_snapshots
            .iter()
            .all(|cache| cache.get().is_none())
    );
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
    assert_eq!(
        relay.receive_batch().receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, b"\xf5"),
            0,
            &mut pending,
            0
        ),
        Err(Disconnect::OVERLOADED)
    );
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
    assert_eq!(
        constrained.receive_batch().receive_bounded(
            &source.peer,
            protocol::envelope(BROADCAST, &protocol::byte_string(&[0x22; 512])),
            0,
            &mut pending,
            1
        ),
        Err(Disconnect::OVERLOADED)
    );
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
    assert_eq!(
        relay.receive_batch().receive_bounded(
            &source.peer,
            protocol::envelope(target.peer.id.load(Relaxed), b"\xf5"),
            0,
            &mut pending,
            1,
        ),
        Err(Disconnect::OVERLOADED)
    );
    assert_eq!(
        data(&output).len(),
        1,
        "observer staging prevented the primary private delivery"
    );
    assert!(observers.iter().all(|(_, output)| data(output).is_empty()));
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
    assert!(Arc::ptr_eq(
        relay.state.read().users.get(&id).unwrap(),
        &replacement.peer
    ));
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
    let registry = relay
        .state
        .try_write()
        .expect("output submission retained membership lock");
    drop(registry);
    let barrier_relay = relay.clone();
    let observer = last.peer.clone();
    let barrier = std::thread::spawn(move || {
        barrier_relay
            .receive(&observer, protocol::request(1, b"\x00"))
            .unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while relay.state.try_read().is_some() {
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
