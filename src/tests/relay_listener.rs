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
