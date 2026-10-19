# Runtime clock call census — 2026-10-18

A short instrumented census identifies where clock calls execute; it does not benchmark a clock optimization. No runtime change is selected.

| Clock site | Fanout only | Mixed traffic |
| --- | ---: | ---: |
| Connection::drive_batched entry | 3,411,652 | 1,502,556 |
| Queued-frame deadline creation | 3,407,872 | 1,335,296 |
| Worker timer processing | 36,206 | 13,388 |
| Worker poll-timeout calculation | 36,206 | 13,388 |
| RoomFanout deadline creation | 1,664 | 652 |
| Other nine instrumented sites | 0 | 0 |
| Untracked calls | 0 | 0 |

The dominant calls are connection-driving time and individual queued-frame admission deadlines. Combined worker-loop calls are only about 1% of instrumented calls in both fixtures. This explains why skipping some discarded poll-timeout calculations was a weak candidate. Counts are not elapsed cost; call frequency does not itself prove where sampled clock cycles originated.

Each measured interval was two seconds with 2,048 readers, four messages per window, four pinned server workers and eight pinned client workers. Mixed adds 64 private pairs. Setup/retirement take additional time. Both passed payload, FIFO, exact counters and resource-retirement gates, with normal client/server exits. No 100,000-user or long-term latency claim follows.

The isolated clean snapshot instruments 14 explicit Instant::now sites in native server.rs, server/socket.rs and relay.rs. The complete site list is sites.json. It does not intercept clock calls inside dependencies or other modules. Cache-aligned per-worker counter slots use relaxed load/store with exactly one writer per slot, under the fixture's single relay and four uniquely named war-io workers. A separate fallback counter catches calls outside those slots; its measured delta is zero. The counters are process-global diagnostic state, not reusable production telemetry. Quiescent ready/measured gates bound the reported deltas.

Instrumentation adds TLS access and counters and changes code layout. Do not compare its latency, CPU or throughput to ordinary builds. The diagnostic patch applies to the clean base commit in summary.json; pre-existing dirty overlays were excluded. The binary SHA-256 is recorded, receipts include full commands and client identity, and the final successful build log is retained. Run python verify.py for offline checks.

The two dominant times are not interchangeable: one checks current connection policy, the other starts a particular queued frame's timeout. Sharing a stale timestamp across recipients could change timeout behavior. Further optimization needs a narrow correctness argument and ordinary release comparisons rather than relaxing deadline policy. Manual services and protected workspace edits were preserved.
