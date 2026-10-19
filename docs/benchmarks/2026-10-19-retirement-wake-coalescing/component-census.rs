
struct CountingRetirement {
    wake: mio::Waker,
    calls: AtomicUsize,
}
impl PeerOutput for CountingRetirement {
    fn send(&self, _: Submission<'_>) -> bool { true }
    fn close(&self, _: Disconnect) {}
    fn supports_retirement(&self) -> bool { true }
    fn wake_retirement(&self) {
        self.calls.fetch_add(1, Relaxed);
        self.wake.wake().unwrap();
    }
}

// Diagnostic only: release-build component timing, no network latency claim.
#[test]
#[ignore]
fn retirement_wake_census() {
    let mut elapsed = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..7 {
        let relay = Relay::new(Config::default()).unwrap();
        let poll = mio::Poll::new().unwrap();
        let output = Arc::new(CountingRetirement {
            wake: mio::Waker::new(poll.registry(), mio::Token(1)).unwrap(),
            calls: AtomicUsize::new(0),
        });
        let first = relay.subscribe_with_output(b"backlog", output.clone()).unwrap();
        let second = relay.subscribe_with_output(b"backlog", output.clone()).unwrap();
        let recorder = Arc::new(Departures::default());
        recorder.0.lock().reserve(FANOUT_QUANTUM + 1001);
        let mut targets: Vec<_> = (0..FANOUT_QUANTUM+1)
            .map(|_| relay.subscribe_with_output(b"backlog", recorder.clone()).unwrap()).collect();
        let mut sources = Vec::new();
        let mut expected = vec![first.peer.id.load(Relaxed); FANOUT_QUANTUM+1];
        for index in 0..1000 {
            let name = format!("wake-{index}");
            let source = relay.subscribe_with_output(name.as_bytes(), output.clone()).unwrap();
            expected.push(source.peer.id.load(Relaxed));
            sources.push(source);
            targets.push(relay.subscribe_with_output(name.as_bytes(), recorder.clone()).unwrap());
        }
        drop(first);
        assert!(relay.has_retirements());
        let began = std::time::Instant::now();
        drop(sources);
        elapsed.push(began.elapsed().as_nanos() as u64);
        let burst_calls = output.calls.load(Relaxed);
        calls.push(burst_calls);
        for _ in 0..1002 { if !relay.has_retirements() { break; } relay.progress_retirements(); }
        assert!(!relay.has_retirements());
        {
            let frames = recorder.0.lock();
            assert_eq!(frames.len(), expected.len());
            for (frame, id) in frames.iter().zip(expected) {
                assert_eq!(frame.to_vec(), protocol::envelope(BROADCAST, &protocol::uint(id)));
            }
        }
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        // Empty -> nonempty must wake again after the previous batch drained.
        drop(second);
        assert!(relay.has_retirements());
        assert_eq!(output.calls.load(Relaxed), burst_calls+1);
        for _ in 0..2 { relay.progress_retirements(); }
        assert!(!relay.has_retirements());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        relay.begin_shutdown();
        drop(targets);
        assert_eq!(relay.counts(), (0, 1));
    }
    println!("WAR_WAKE_CENSUS={}", serde_json::json!({"source_drops":1000,"elapsed_ns":elapsed,"wake_calls_including_initial":calls}));
}
