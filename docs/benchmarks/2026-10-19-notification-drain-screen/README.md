# Reject notification queue swapping

Decision: unselected. The native runtime remains unchanged.

A two-second mixed profile with 2,048 fanout readers and 64 private pairs showed fanout enqueue/notification work among visible costs; `Socket::notify` accounted for 4.26% of self user-cycle samples on the sender worker. The profile lost no samples. This identifies an experiment target, not proof of mutex contention. Raw perf data remains at `artifacts/native-pending-profile.perf.data`; the text report and diagnostic receipt are retained here.

The owner currently holds its pending-queue mutex while resolving tokens and scheduling sockets. The candidate swaps that queue into a worker-local reusable `VecDeque`, then drains outside the mutex. It retains notification deduplication, token validation, FIFO socket queues, bounded drive batches and shared payload storage. It adds one queue buffer per worker and may permit additional wakeups while that owner is already active.

Both executables were freshly compiled from the same clean source snapshot and Cargo target directory, sequentially, excluding pre-existing user overlays. Only the archived `src/server.rs` patch differs. The candidate passed all 106 library tests and Clippy with warnings denied.

Four ordinary runs used control/candidate/candidate/control order. Each offered traffic for two seconds, plus setup and checked retirement, with four pinned server workers, eight pinned client workers on disjoint CPUs, 1 KiB byte-string values, 2,048 fanout readers, four messages per fanout window and 64 private pairs. Inspection and churn were disabled. Close windows of 32 shortened exhaustive retirement outside measurement. No builds or profiling overlapped these ordinary runs.

| Variant | CPU seconds / million deliveries | Private million messages/s | Fanout million deliveries/s | Worst private ms | Worst fanout window ms |
| --- | --- | --- | --- | --- | --- |
| Control | 1.225 | 1.391 | 0.728 | 3.102 | 13.821 |
| Candidate | 1.259 | 1.481 | 0.592 | 3.498 | 17.018 |
| Candidate | 1.217 | 1.272 | 0.854 | 3.501 | 12.495 |
| Control | 1.268 | 1.288 | 0.739 | 3.901 | 16.808 |

CPU/delivery ranges overlap. The first candidate gains private throughput while losing fanout throughput; the second reverses that balance. Latency maxima do not consistently improve. These unpaced workloads vary their traffic mix, so normalization is descriptive rather than a fixed-work causal comparison. The short screen provides no reason to retain the extra buffer. It establishes neither a universal regression nor a 100,000-user fairness result.

All four ordinary fixtures passed exact body/FIFO, counter and resource-retirement checks, with normal client/server exits. The existing manual services and three unrelated dirty source files were preserved. The candidate patch is retained only as experiment evidence. Run `python verify.py` from this directory for an offline receipt audit.
