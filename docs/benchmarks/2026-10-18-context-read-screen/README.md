# Real-context buffered read — short screen and 100k rejection

Decision: unselected. Preserve the existing inspection receive helper. An actual-task-context first poll avoids the no-op waker of now_or_never, but its modest small-fixture CPU improvement does not survive the full offered-load workload.

Source inspection of the pinned WebSocket adapter shows every read poll registers the supplied read waker in two proxy slots. The prior no-op probe can replace the task waker and need restoration on fallback. This variant uses poll_fn to perform one immediate poll with the real task context. An already-ready frame returns directly; a pending read uses timeout_at with the five-second absolute deadline captured before probing. No packet bytes or server code change. This is a hypothesis about one overhead, not proof that waker replacement caused the entire previous result.

Four sequential fresh two-second pure-fanout fixtures use the same frozen native server, 2,048 readers, window four, four pinned server workers and eight pinned client workers. Order is control/candidate/candidate/control. Client CPU-s per million checked deliveries: control 2.558/2.512; candidate 2.458/2.476. Throughput and maxima overlap, so the small screen is only preliminary.

Ten isolated client tests pass in five seconds: existing corruption/acknowledgement tests plus ready-frame FIFO, a partial frame across a pending read, a real five-second timeout, unexpected data type and peer Close. The partial-frame test explicitly polls the receive future to Pending before its delayed tail completes. The timeout test retains an outer six-second failure guard. The release binary was frozen before inserting the cfg(test) module; its exact main source snapshot/hash is retained separately from the tested source.

The deciding comparison uses two ordinary six-second offered workloads, control then candidate: 100,000 private users, 8,192 fanout readers, 4 Hz private traffic/churn, 16 Hz fanout with four messages per window, two CBOR inspectors and explicit pinning of all sixteen client workers. Setup and exhaustive retirement add wall time. Neither run overlaps a build, profile or another workload.

| Metric | Control | Candidate |
| --- | ---: | ---: |
| Completed/offered windows | 96/96 | 96/96 |
| Sidecar CPU-seconds | 18.44 | 19.70 |
| Server CPU-seconds | 20.57 | 21.23 |
| Median private user maximum ms | 1.405 | 1.516 |
| Population p99 private maximum ms | 5.013 | 4.790 |
| Worst private maximum ms | 7.397 | 6.521 |
| Median recipient fanout maximum ms | 65.746 | 73.306 |
| Worst recipient fanout maximum ms | 75.223 | 83.339 |
| Worst intended-time fanout maximum ms | 77.024 | 116.139 |

The candidate uses more client/server CPU and has worse fanout tails. Private tails improve slightly while the median worsens. These two finite trials do not establish causality or universal regression, but they do not justify adopting the technique for the targeted mixed population. Per-user private p99 equals maximum at 24 observations. Exact fanout has 384 observations per reader. Client/kernel scheduling is included; these are not hop-only or long-term SLAs.

All six fixtures pass payload/FIFO/counter/resource checks and normal exits. Both large runs have 2.4 million private observations and 3,145,728 fanout deliveries, checked source/affinity/counters and zero final live reservations/queue capacity. Every CSV and timeline is compressed losslessly and matched to its original decoded hash. The weaker small screen did not replace full-population acceptance.

Build records, both release main sources, reconstructable candidate patch/test module, test log, small receipts and complete large CSV/timeline matrices are retained. Small receipt source contexts are not binary/source association; builds.json records actual frozen source/executable identities. Restore read_tests.rs under examples/inspection/ when reconstructing the test patch. Run python verify.py for offline checks. No selected source, default client, native runtime, protected overlay or manual service changed.
