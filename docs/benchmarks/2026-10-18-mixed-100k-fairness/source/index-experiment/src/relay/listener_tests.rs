use super::*;

fn relay() -> Arc<Relay> {
    Relay::new(Config {
        auth_password: Some("listeners-test".into()),
        ..Config::default()
    })
    .unwrap()
}

fn joined(relay: &Arc<Relay>, channel: &[u8]) -> Subscription {
    let mut subscription = relay.subscribe(channel).unwrap();
    let id = subscription.registration.peer.id.load(Relaxed);
    take(&mut subscription, &protocol::uint(id));
    subscription
}

fn take(subscription: &mut Subscription, expected: &Bytes) -> Envelope {
    let packet = subscription.outgoing.try_recv().unwrap();
    assert_eq!(
        [packet.frame.prefix(), packet.frame.payload()].concat(),
        expected.as_ref()
    );
    packet
}

fn empty(subscription: &mut Subscription) {
    assert!(matches!(
        subscription.outgoing.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
}

fn authenticate(relay: &Relay, subscription: &mut Subscription) {
    relay
        .receive(
            &subscription.registration.peer,
            protocol::request(0, &protocol::text("listeners-test")),
        )
        .unwrap();
    take(subscription, &protocol::reply(0, &[0xf5]));
}

fn listen(relay: &Relay, subscription: &mut Subscription, flags: u8) {
    relay
        .receive(
            &subscription.registration.peer,
            protocol::request(1, &protocol::uint(u64::from(flags))),
        )
        .unwrap();
    take(
        subscription,
        &protocol::reply(1, &protocol::uint(u64::from(flags & 7))),
    );
}

fn assign(relay: &Relay, subscription: &mut Subscription, id: u64) {
    relay
        .receive(
            &subscription.registration.peer,
            protocol::request(3, &protocol::uint(id)),
        )
        .unwrap();
    take(subscription, &protocol::reply(3, &protocol::uint(id)));
}

#[test]
fn listener_mask_changes_preserve_all_three_scopes_and_shared_payloads() {
    let relay = relay();
    let mut sender = joined(&relay, b"data");
    let mut target = joined(&relay, b"data");
    let mut observer = joined(&relay, GLOBAL_CHANNEL);
    let mut ordinary = joined(&relay, b"ordinary");
    authenticate(&relay, &mut observer);
    authenticate(&relay, &mut ordinary);
    listen(&relay, &mut ordinary, 7);
    let sender_id = sender.registration.peer.id.load(Relaxed);
    let target_id = target.registration.peer.id.load(Relaxed);
    let value = protocol::byte_string(&[0x5a; 2048]);
    let expected = protocol::envelope(sender_id, &value);
    for flags in [0, 1, 2, 3, 4, 5, 6, 7, 7, 6, 5, 4, 3, 2, 1, 0, 8, 255, 0] {
        listen(&relay, &mut observer, flags);
        let input = protocol::envelope(target_id, &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&sender.registration.peer, input).unwrap();
        let packet = take(&mut target, &expected);
        assert_eq!(packet.frame.payload().as_ptr(), pointer);
        if flags & PRIVATE_MESSAGE != 0 {
            let observed = take(&mut observer, &expected);
            assert_eq!(observed.frame.payload().as_ptr(), pointer);
        }
        empty(&mut observer);
        empty(&mut ordinary);
        relay
            .receive(
                &sender.registration.peer,
                protocol::envelope(BROADCAST, &[0xf5]),
            )
            .unwrap();
        let broadcast = protocol::envelope(sender_id, &[0xf5]);
        take(&mut target, &broadcast);
        if flags & CHANNEL_MESSAGE != 0 {
            take(&mut observer, &broadcast);
        }
        empty(&mut observer);
        empty(&mut ordinary);
        let transient = joined(&relay, b"transient");
        let departed_id = transient.registration.peer.id.load(Relaxed);
        drop(transient);
        if flags & DISCONNECT_MESSAGE != 0 {
            take(
                &mut observer,
                &protocol::envelope(BROADCAST, &protocol::uint(departed_id)),
            );
        }
        empty(&mut observer);
        empty(&mut ordinary);
        empty(&mut sender);
    }
    drop((observer, ordinary, sender, target));
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn listener_identity_survives_assignment_takeover_and_delayed_cleanup() {
    let relay = relay();
    let source = joined(&relay, b"source");
    let mut old = joined(&relay, GLOBAL_CHANNEL);
    let mut next = joined(&relay, GLOBAL_CHANNEL);
    authenticate(&relay, &mut old);
    authenticate(&relay, &mut next);
    listen(&relay, &mut old, 7);
    assign(&relay, &mut old, 42);
    let expected = protocol::envelope(source.registration.peer.id.load(Relaxed), &[0xf5]);
    relay
        .receive(&source.registration.peer, protocol::envelope(42, &[0xf5]))
        .unwrap();
    take(&mut old, &expected);
    empty(&mut old); // Target plus listener still receives exactly one copy.
    listen(&relay, &mut next, 7);
    assign(&relay, &mut next, 42);
    assert_eq!(*old.close.borrow(), Some(Disconnect::TAKEN));
    let old_weak = Arc::downgrade(&old.registration.peer);
    drop(old);
    assert!(old_weak.upgrade().is_none());
    take(
        &mut next,
        &protocol::envelope(BROADCAST, &protocol::uint(42)),
    );
    relay
        .receive(&source.registration.peer, protocol::envelope(42, &[0xf5]))
        .unwrap();
    take(&mut next, &expected);
    empty(&mut next);
    assign(&relay, &mut next, 99);
    relay
        .receive(&source.registration.peer, protocol::envelope(42, &[0xf5]))
        .unwrap();
    empty(&mut next);
    listen(&relay, &mut next, 255);
    relay
        .receive(&source.registration.peer, protocol::envelope(99, &[0xf5]))
        .unwrap();
    take(&mut next, &expected);
    empty(&mut next);
    let next_weak = Arc::downgrade(&next.registration.peer);
    drop(next);
    assert!(next_weak.upgrade().is_none());
    drop(source);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

struct UnlockedObserver {
    relay: std::sync::Weak<Relay>,
    received: parking_lot::Mutex<Option<Envelope>>,
}

impl PeerOutput for UnlockedObserver {
    fn send(&self, packet: Submission<'_>) -> bool {
        if packet.frame.payload().len() > 100 {
            assert!(
                self.relay.upgrade().unwrap().state.try_write().is_some(),
                "observer output must release the membership registry"
            );
            self.received.lock().replace(packet.into_owned());
        }
        true
    }
    fn close(&self, _: Disconnect) {}
}

fn observer_output_releases_membership(flags: u8) {
    let relay = relay();
    let source = joined(&relay, b"data");
    let mut target = joined(&relay, b"data");
    let probe = Arc::new(UnlockedObserver {
        relay: Arc::downgrade(&relay),
        received: parking_lot::Mutex::new(None),
    });
    let observer = relay
        .subscribe_with_output(GLOBAL_CHANNEL, probe.clone())
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(0, &protocol::text("listeners-test")),
        )
        .unwrap();
    relay
        .receive(
            &observer.peer,
            protocol::request(1, &protocol::uint(u64::from(flags))),
        )
        .unwrap();
    let value = protocol::byte_string(&vec![0xa5; 8192]);
    let destination = if flags == CHANNEL_MESSAGE {
        BROADCAST
    } else {
        target.registration.peer.id.load(Relaxed)
    };
    let input = protocol::envelope(destination, &value);
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    let expected = protocol::envelope(source.registration.peer.id.load(Relaxed), &value);
    relay.receive(&source.registration.peer, input).unwrap();
    drop(take(&mut target, &expected));
    let held = probe.received.lock().take().unwrap();
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    drop(observer);
    drop(source);
    drop(target);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn channel_observer_output_releases_membership_and_retains_the_original_body() {
    observer_output_releases_membership(CHANNEL_MESSAGE);
}

#[test]
fn private_observer_output_releases_membership_and_retains_the_original_body() {
    observer_output_releases_membership(PRIVATE_MESSAGE);
}

struct PausedObserver {
    entered: std::sync::mpsc::SyncSender<()>,
    resume: parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
    sent: parking_lot::Mutex<Vec<Vec<u8>>>,
}

impl PeerOutput for PausedObserver {
    fn send(&self, packet: Submission<'_>) -> bool {
        if packet.frame.payload().len() > 100 {
            self.entered.send(()).unwrap();
            self.resume
                .lock()
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
        }
        self.sent.lock().push(packet.frame.to_vec());
        true
    }
    fn close(&self, _: Disconnect) {}
}

#[test]
fn observer_permission_reply_waits_for_selected_output_while_unrelated_membership_progresses() {
    for flags in [CHANNEL_MESSAGE, PRIVATE_MESSAGE] {
        let relay = relay();
        let source = joined(&relay, b"data");
        let mut target = joined(&relay, b"data");
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
        let probe = Arc::new(PausedObserver {
            entered: entered_tx,
            resume: parking_lot::Mutex::new(resume_rx),
            sent: parking_lot::Mutex::new(Vec::new()),
        });
        let observer = relay
            .subscribe_with_output(GLOBAL_CHANNEL, probe.clone())
            .unwrap();
        relay
            .receive(
                &observer.peer,
                protocol::request(0, &protocol::text("listeners-test")),
            )
            .unwrap();
        relay
            .receive(
                &observer.peer,
                protocol::request(1, &protocol::uint(u64::from(flags))),
            )
            .unwrap();
        probe.sent.lock().clear();
        let value = protocol::byte_string(&vec![0x5a; 8192]);
        let expected = protocol::envelope(source.registration.peer.id.load(Relaxed), &value);
        let destination = if flags == CHANNEL_MESSAGE {
            BROADCAST
        } else {
            target.registration.peer.id.load(Relaxed)
        };
        let input = protocol::envelope(destination, &value);
        let route_relay = relay.clone();
        let sender = source.registration.peer.clone();
        let route = std::thread::spawn(move || route_relay.receive(&sender, input).unwrap());
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        // Admission and complete retirement work while observer output is parked.
        let transient = joined(&relay, b"unrelated");
        drop(transient);
        let index = flags.trailing_zeros() as usize;
        assert!(relay.listeners[index].try_write().is_none());
        let update_relay = relay.clone();
        let recipient = observer.peer.clone();
        let update = std::thread::spawn(move || {
            update_relay
                .receive(&recipient, protocol::request(1, &[0]))
                .unwrap()
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        // The flag changes before the writer waits for the retained index guard.
        while observer.peer.listener.load(Relaxed) != 0 {
            assert!(std::time::Instant::now() < until);
            std::thread::yield_now();
        }
        assert!(
            probe.sent.lock().is_empty(),
            "permission reply overtook selected output"
        );
        resume_tx.send(()).unwrap();
        route.join().unwrap();
        update.join().unwrap();
        assert_eq!(
            *probe.sent.lock(),
            vec![expected.to_vec(), protocol::reply(1, &[0]).to_vec()]
        );
        drop(take(&mut target, &expected));
        assert_eq!(relay.state.read().listener_count(flags), 0);
        assert!(relay.listeners[index].read().is_empty());
        drop(observer);
        drop(source);
        drop(target);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}
