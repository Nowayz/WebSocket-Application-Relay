use super::*;
use std::sync::Weak;

struct Probe {
    relay: Weak<Relay>,
    received: parking_lot::Mutex<Option<Envelope>>,
}

impl PeerOutput for Probe {
    fn send(&self, packet: Submission<'_>) -> bool {
        if packet.frame.payload().len() > 100 {
            let relay = self.relay.upgrade().unwrap();
            assert!(
                relay.registry_unlocked(),
                "broadcast output must run after releasing the registry"
            );
            self.received.lock().replace(packet.into_owned());
        }
        true
    }
    fn close(&self, _: Disconnect) {}
}

#[test]
fn broadcast_output_releases_the_registry_and_retains_the_original_body() {
    let relay = Relay::new(Config::default()).unwrap();
    let probe = Arc::new(Probe {
        relay: Arc::downgrade(&relay),
        received: parking_lot::Mutex::new(None),
    });
    let target = relay.subscribe_with_output(b"view", probe.clone()).unwrap();
    let mut source = relay.subscribe(b"view").unwrap();
    source.outgoing.try_recv().unwrap();
    let value = protocol::byte_string(&vec![0x5a; 8192]);
    let input = protocol::envelope(BROADCAST, &value);
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&source.registration.peer, input).unwrap();
    let held = probe.received.lock().take().unwrap();
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    drop(target);
    drop(source);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn membership_changes_invalidate_views_without_changing_captured_owners() {
    let relay = Relay::new(Config::default()).unwrap();
    let mut source = relay.subscribe(b"view").unwrap();
    let mut target = relay.subscribe(b"view").unwrap();
    source.outgoing.try_recv().unwrap();
    target.outgoing.try_recv().unwrap();
    let value = protocol::byte_string(&vec![0xa5; 8192]);
    let input = protocol::envelope(BROADCAST, &value);
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&source.registration.peer, input).unwrap();
    let held = target.outgoing.try_recv().unwrap();
    let original = relay.channel(b"view").unwrap().cached_view().unwrap();
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[7]),
        )
        .unwrap();
    drop(target.outgoing.try_recv().unwrap());
    assert!(Arc::ptr_eq(
        &original,
        &relay.channel(b"view").unwrap().cached_view().unwrap()
    ));
    let mut late = relay.subscribe(b"view").unwrap();
    late.outgoing.try_recv().unwrap();
    assert!(relay.channel(b"view").unwrap().cached_view().is_none());
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[9]),
        )
        .unwrap();
    drop(target.outgoing.try_recv().unwrap());
    drop(late.outgoing.try_recv().unwrap());
    let replacement = relay.channel(b"view").unwrap().cached_view().unwrap();
    assert!(!Arc::ptr_eq(&original, &replacement));
    assert_eq!(original.len(), 2);
    assert_eq!(replacement.len(), 3);
    let removed = Arc::downgrade(&target.registration.peer);
    drop(target);
    assert!(relay.channel(b"view").unwrap().cached_view().is_none());
    assert!(
        removed.upgrade().is_some(),
        "captured views must keep their owners"
    );
    drop(original);
    drop(replacement);
    assert!(removed.upgrade().is_none());
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    drop(source);
    drop(late);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn uncaptured_membership_updates_reuse_metadata_and_retire_departed_owners() {
    let relay = Relay::new(Config::default()).unwrap();
    let mut source = relay.subscribe(b"reuse").unwrap();
    let mut target = relay.subscribe(b"reuse").unwrap();
    source.outgoing.try_recv().unwrap();
    target.outgoing.try_recv().unwrap();
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[7]),
        )
        .unwrap();
    let held = target.outgoing.try_recv().unwrap();
    let view = Arc::as_ptr(&relay.channel(b"reuse").unwrap().cached_view().unwrap());
    let mut late = relay.subscribe(b"reuse").unwrap();
    late.outgoing.try_recv().unwrap();
    {
        let current = relay
            .channel(b"reuse")
            .unwrap()
            .cached_view()
            .expect("an uncaptured view must survive a membership update");
        assert_eq!(Arc::as_ptr(&current), view);
        assert_eq!(current.len(), 3);
    }
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[9]),
        )
        .unwrap();
    assert_eq!(target.outgoing.try_recv().unwrap().frame.payload(), &[9]);
    assert_eq!(late.outgoing.try_recv().unwrap().frame.payload(), &[9]);
    let removed = Arc::downgrade(&late.registration.peer);
    drop(late);
    assert!(
        removed.upgrade().is_none(),
        "the cache must retire a departed peer"
    );
    {
        let current = relay.channel(b"reuse").unwrap().cached_view().unwrap();
        assert_eq!(Arc::as_ptr(&current), view);
        assert_eq!(current.len(), 2);
    }
    assert_eq!(held.frame.payload(), &[7]);
    drop((source, target, held));
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn a_retained_room_releases_sparse_recipient_capacity_after_a_membership_wave() {
    struct Discard;
    impl PeerOutput for Discard {
        fn send(&self, _: Submission<'_>) -> bool {
            true
        }
        fn close(&self, _: Disconnect) {}
    }
    let relay = Relay::new(Config::default()).unwrap();
    let output: Arc<dyn PeerOutput> = Arc::new(Discard);
    let source = relay
        .subscribe_with_output(b"capacity", output.clone())
        .unwrap();
    let mut peers: Vec<_> = (0..1024)
        .map(|_| {
            relay
                .subscribe_with_output(b"capacity", output.clone())
                .unwrap()
        })
        .collect();
    relay
        .receive(&source.peer, protocol::envelope(BROADCAST, &[7]))
        .unwrap();
    let late = relay.subscribe_with_output(b"capacity", output).unwrap();
    let peak = relay
        .channel(b"capacity")
        .unwrap()
        .cached_view()
        .expect("a unique view must grow in place")
        .capacity();
    drop(late);
    while peers.len() > 1 {
        peers.pop();
    }
    let current = relay.channel(b"capacity").unwrap().cached_view().unwrap();
    assert_eq!(current.len(), 2);
    assert!(current.capacity() <= 256 && current.capacity() < peak);
    drop(current);
    drop((source, peers));
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
