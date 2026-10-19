# One reader timeout per fanout window — 2026-10-18

Decision: retain the current benchmark reader. A single timeout around a complete fanout window saves client CPU in the short fanout-only screen, but does not establish a whole-workload improvement at 100,000 users. The candidate remains isolated and unselected; no runtime or default client changed.

The existing native+client attribution highlighted timeout/receive polling on the sidecar. Prior buffered-read experiments still retained a timer per packet and failed full-population selection. This experiment instead wraps the reader's complete message loop in one five-second timeout, using the same binary-message extraction helper without another nested timeout per message. Departure and other reads keep their existing timeout. Every payload/sender/FIFO comparison, receive counter, timestamp and exact sample remains inside the loop. The controller's existing five-second acknowledgement deadline is unchanged.

The candidate reader has a stricter complete-window deadline rather than a fresh five seconds for each packet. No deadline is disabled, no first-poll/no-op-waker fast path is used, no application body is copied, and no packet validation is skipped. The helper extraction and timeout boundary can also alter compiler layout and scheduling; a smaller timer count alone does not prove lower whole-system cost.

Both clients were built from clean snapshots of the same base, with only the archived two-file candidate patch differing. The frozen native server was identical in every trial. It includes the selected SIMD text and shutdown-drain changes. All builds and tests finished before timed traffic; candidates ran sequentially without overlapping profiles or other benchmarks.

Four two-second fanout-only runs used 2,048 readers, window four, 1 KiB byte values, four native workers and eight client workers on disjoint pinned CPU sets. The order was control/candidate/candidate/control. Results below are independent process values, not pooled samples.

| Ordinary short screen | Control | Candidate |
| --- | --- | --- |
| Client CPU-s / million verified deliveries | 2.412, 2.441 | 2.216, 2.278 |
| Native CPU-s / million verified deliveries | 1.107, 1.149 | 1.087, 1.156 |
| Fanout maximum | 6.902, 7.866 ms | 6.667, 9.732 ms |

Client CPU is lower in both candidate trials, while native CPU overlaps and maximum latency is mixed. This unpaced screen is not the full-population acceptance workload.

The matched full-population pair ran control then candidate, each with a six-second measured phase, 100,000 private users, 8,192 fanout readers, 4 Hz private probes, churn and 16 Hz fanout windows. Setup and checked retirement take additional time outside measurement. Both completed 96/96 offered windows, 2.4 million private observations and 3,145,728 checked fanout deliveries. Both have clean process exits, no generator period overrun and zero final connection/session/queue/read-buffer/output-capacity gauges, with zero routing payload copied bytes.

| Full-population observation | Control | Candidate |
| --- | --- | --- |
| Sidecar measured CPU seconds | 18.48 | 19.17 |
| Native measured CPU seconds | 20.45 | 21.03 |
| Median of private-user observed maxima | 1.473 ms | 1.498 ms |
| p99 of private-user observed maxima | 13.698 ms | 19.227 ms |
| Worst private observation | 17.119 ms | 22.223 ms |
| Worst intended-send private observation | 18.392 ms | 24.069 ms |
| Median of fanout-reader observed maxima | 67.382 ms | 54.499 ms |
| Worst fanout observation | 77.386 ms | 64.086 ms |
| Worst intended-send fanout observation | 91.631 ms | 65.534 ms |

The candidate's lower fanout tail comes with worse private tails and higher measured CPU in this pair. Single full-population trials do not prove that the code change caused each difference, but the combined evidence does not justify selection for consistently responsive service to every user. No extra tuning or repeated long campaign was pursued. These are complete end-to-end local observations, not isolated server-hop measurements or long-term latency guarantees. Each private user has only 24 observations, so its sample p99 equals its maximum.

All eight candidate example tests pass in five seconds, including non-primary reader/global-observer corruption, departure checks and the controller's fixed acknowledgement deadline. The release client builds. A separate new-reader partial-frame/cancellation test was not added after the screen rejected adoption; existing controller deadline coverage does not independently prove every cancellation case in the changed reader wrapper.

`builds.json` pins the clean base, both clients and the common server. The isolated candidate patch, exact runner scripts, logs, full receipts, private/fanout CSVs and per-second windows are losslessly retained. Run `python docs/benchmarks/2026-10-18-window-timeout-screen/verify.py` to independently check hashes, counts, population summaries and offered-load/resource gates without starting traffic.

Manual services, default executables and the three pre-existing dirty source files remain unchanged.
