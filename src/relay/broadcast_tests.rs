use super::*;
use std::sync::Weak;

struct Probe {
    relay: Weak<Relay>,
    received: parking_lot::Mutex<Option<Envelope>>,
}

impl PeerOutput for Probe {
    fn send(&self, packet: Envelope) -> bool {
        if packet.frame.payload().len() > 100 {
            let relay = self.relay.upgrade().unwrap();
            assert!(
                relay.state.try_write().is_some(),
                "broadcast output must run after releasing the registry"
            );
            self.received.lock().replace(packet);
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
    let original = relay.state.read().channels[b"view".as_slice()]
        .broadcast
        .get()
        .unwrap()
        .clone();
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[7]),
        )
        .unwrap();
    drop(target.outgoing.try_recv().unwrap());
    assert!(Arc::ptr_eq(
        &original,
        relay.state.read().channels[b"view".as_slice()]
            .broadcast
            .get()
            .unwrap()
    ));
    let mut late = relay.subscribe(b"view").unwrap();
    late.outgoing.try_recv().unwrap();
    assert!(
        relay.state.read().channels[b"view".as_slice()]
            .broadcast
            .get()
            .is_none()
    );
    relay
        .receive(
            &source.registration.peer,
            protocol::envelope(BROADCAST, &[9]),
        )
        .unwrap();
    drop(target.outgoing.try_recv().unwrap());
    drop(late.outgoing.try_recv().unwrap());
    let replacement = relay.state.read().channels[b"view".as_slice()]
        .broadcast
        .get()
        .unwrap()
        .clone();
    assert!(!Arc::ptr_eq(&original, &replacement));
    assert_eq!(original.len(), 2);
    assert_eq!(replacement.len(), 3);
    let removed = Arc::downgrade(&target.registration.peer);
    drop(target);
    assert!(
        relay.state.read().channels[b"view".as_slice()]
            .broadcast
            .get()
            .is_none()
    );
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
