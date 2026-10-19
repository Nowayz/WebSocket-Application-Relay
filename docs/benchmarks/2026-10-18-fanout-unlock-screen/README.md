# Fanout socket-lock scope screen

Rejected: releasing the socket-state guard immediately before the common queued-output notification does not establish a repeatable latency or CPU improvement. Keep the current runtime. No executable promotion or production code change follows.

A separate two-second current-native 100,000-user profile, using the corrected lightweight monitor and zero independent churn workers, records 4,713 user-cycle samples with zero reported loss. Self sample shares are 19.93% `Connection::drive_batched`, 14.82% `ReceiveBatch::receive_bounded`, 5.24% `Peer::enqueue_fanout`, 3.64% `Socket::notify`, and 3.51% `Socket::send_fanout`. The fanout submission/notification share motivates a narrow experiment; these mixed-workload cycle samples are neither a latency decomposition nor a measure of kernel time or mutex wait duration. Profiling is separate from ordinary comparisons.

The prototype adds only `drop(state)` before the final `if first { self.notify(); }` in `Socket::submit`. The queue is published under the same state mutex, then the existing atomic exchange, pending-queue mutex and wake mechanism run. No flag operations, queue representations, message ordering, budgets, timeouts, payload lifetimes or fatal/Close paths change. A consumer may drain before the delayed notification; that can create a redundant wake. This is an experiment, not a selected concurrency redesign. Both source files and the complete candidate build source-hash map are archived.

Control/unlock/unlock/control each run two seconds of traffic; eight measured seconds total. Setup and exhaustive checked teardown add wall time. All hold 100,000 private users in 6,250 rooms of 16, with four 1 KiB probes/user/second and eight recorded observations per user. A separate room has 8,192 readers, four 1 KiB messages/window at 16 Hz: 32 windows and 128 observations per reader. Independent churn workers, inspectors, additional private pairs and fanout-audience membership churn are disabled. No TLS. Eight server workers and both eight-worker clients use the same disjoint physical CPU sets, with checked per-thread affinity. Commands and identities are retained. No timing run overlaps a build, test, profile or compression task.

| Run | CPU-s at gates | Private worst ms | Fanout worst ms | Last-send to all ACKs worst ms |
| --- | ---: | ---: | ---: | ---: |
| 0 control | 6.29 | 3.883 | 51.919 | 49.602 |
| 1 unlock | 6.54 | 4.092 | 53.065 | 50.658 |
| 2 unlock | 6.50 | 3.701 | 51.642 | 49.252 |
| 3 control | 6.60 | 3.610 | 51.322 | 48.896 |

All four trials pass correctness, complete 32/32 offered windows, and exit zero. Every private user and fanout reader has the expected observation count; full CSVs are retained. Exact lifecycle/message/byte counters reconcile, routing-copy counters stay zero, and all connection/session/queue/read-storage gauges return to zero with only the root channel retained. The runner also checks final descriptor return to baseline. CPU covers existing ready/measured gates and settling, not exactly two traffic seconds.

Candidate CPU 6.50–6.54 seconds overlaps control 6.29–6.60. Candidate private maxima 3.70–4.09 ms overlap control 3.61–3.88. Candidate fanout maxima 51.64–53.07 ms overlap control 51.32–51.92. Neither repeat provides a compelling fanout-tail improvement. Four short local trials cannot establish a long-term per-user p99, an Internet latency SLA or general equivalence. These measurements provide no reason to retain this change.

The clean candidate passes all 92 library tests in 0.51 seconds and Clippy with warnings denied. The existing tests cover partial writes, body borrowing, bounded queues, fairness, ownership, Close handling and shutdown. These tests and finite traffic checks do not prove all possible thread interleavings. No extra synchronization or test-only scheduling hooks were introduced.

The source base is e853c5e. The control binary is the previously frozen selected native source through b92da66; subsequent changes before this experiment are tooling, tests and JavaScript client changes. Runtime source overlays are excluded from the clean snapshot. Candidate build evidence records a fresh release executable and all snapshot source hashes. Receipts pin both server hashes and identical client hashes. Existing manual services and the three unrelated dirty source files are preserved.

Run `python docs/benchmarks/2026-10-18-fanout-unlock-screen/verify.py` for offline evidence verification. The profile is diagnostic evidence only and is excluded from ordinary performance comparisons. Full profile data, reports, commands, receipts, per-user CSVs, source variants, candidate build record and validation logs are compressed in `raw/`. The next investigation should separate publisher-side fanout work from reader/client scheduling before adding another server mechanism.
