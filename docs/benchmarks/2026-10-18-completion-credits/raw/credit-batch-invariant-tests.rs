
#[cfg(test)]
mod completion_batch_tests {
    use super::*;

    fn packet(peer: &Peer, frame: &Frame) -> Envelope {
        peer.reserve(frame).unwrap().unwrap().into_owned()
    }

    #[test]
    fn completed_credits_remain_charged_until_batch_drop_and_pending_survives_retirement() {
        let relay = Relay::new(Config::default()).unwrap();
        let mut sub = relay.subscribe(b"completion").unwrap();
        drop(sub.outgoing.try_recv().unwrap());
        let frame = Frame::envelope(42, protocol::byte_string(&[0x5a; 32]));
        let size = frame.len();
        let first = packet(&sub.registration.peer, &frame);
        let second = packet(&sub.registration.peer, &frame);
        let pending = packet(&sub.registration.peer, &frame);
        let mut batch = CompletionBatch::default();
        batch.complete(first);
        batch.complete(second);
        assert_eq!(sub.registration.peer.budget.used.load(Relaxed), 3 * size);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 3 * size);
        drop(batch);
        assert_eq!(sub.registration.peer.budget.used.load(Relaxed), size);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), size);
        drop(sub);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), size);
        drop(pending);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn changing_budget_owner_releases_only_the_previous_completed_group() {
        let relay = Relay::new(Config::default()).unwrap();
        let mut a = relay.subscribe(b"a").unwrap();
        let mut b = relay.subscribe(b"b").unwrap();
        drop(a.outgoing.try_recv().unwrap());
        drop(b.outgoing.try_recv().unwrap());
        let frame = Frame::integer(42);
        let size = frame.len();
        let a1 = packet(&a.registration.peer, &frame);
        let a2 = packet(&a.registration.peer, &frame);
        let b1 = packet(&b.registration.peer, &frame);
        let mut batch = CompletionBatch::default();
        batch.complete(a1);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 3 * size);
        batch.complete(b1);
        assert_eq!(a.registration.peer.budget.used.load(Relaxed), size);
        assert_eq!(b.registration.peer.budget.used.load(Relaxed), size);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 2 * size);
        batch.complete(a2);
        assert_eq!(b.registration.peer.budget.used.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), size);
        drop(batch);
        assert_eq!(a.registration.peer.budget.used.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn completed_credits_are_released_during_unwinding() {
        let relay = Relay::new(Config::default()).unwrap();
        let mut sub = relay.subscribe(b"panic").unwrap();
        drop(sub.outgoing.try_recv().unwrap());
        let frame = Frame::integer(42);
        let owned = packet(&sub.registration.peer, &frame);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut batch = CompletionBatch::default();
            batch.complete(owned);
            panic!("injected completion unwind");
        }));
        assert!(result.is_err());
        assert_eq!(sub.registration.peer.budget.used.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}
