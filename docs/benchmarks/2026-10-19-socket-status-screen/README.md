# Socket status lock consolidation screen — 2026-10-19

Decision: leave the runtime unchanged. One fewer mutex acquisition in connection driving did not establish a repeatable whole-workload benefit. The candidate remains only in the archived experiment snapshot. No production service, convenience executable, unrelated dirty source or runtime default was changed.

A fresh two-second mixed-load user-cycle profile of the preceding frozen committed-source binary passes all correctness, retirement and 32 offered-window gates. It records roughly 5,000 samples with zero lost samples. Aggregated self samples assign 18.22% to Connection::drive_batched, 14.87% to ReceiveBatch::receive_bounded, 6.84% to Peer::enqueue_fanout, 5.90% to Worker::schedule and 5.88% to Worker::run. These are sampled CPU shares, not wall-time or latency attribution, and do not prove that locks cause the observed tails. The raw local perf data path and hash remain in profile.json; the text report is retained here. Profiled timing is excluded from the comparison.

The small candidate reads fatal status, requested Close and write deadline under the existing first socket-state lock in drive_websocket. It removes the later independent deadline lock. It adds no queue, allocation, copy, dependency or configuration and does not keep the guard across network I/O. The standalone deadline helper remains for other callers. Existing concurrent writes can change state after either snapshot; this experiment does not introduce an atomic transaction across the rest of connection driving. The 126 library tests pass in 0.50 seconds, including short-write/FIFO ownership, backpressure, output-only policy deferral and deadline boundary coverage.

Both release binaries are frozen from clean source snapshots with Cargo fresh:false and source/binary hashes in their build receipts. The candidate differs only in the retained six-line patch; both exclude the three unrelated dirty runtime files. All builds and tests finish before ordinary timing. The same frozen clients and workload are used sequentially in control/candidate/candidate/control order: 100,000 private users at 4 Hz, 8,192 observer recipients, four-message fanout windows at 16 Hz and eight 16-Hz churn workers. Each traffic phase lasts two seconds. Setup, independent population checks and orderly teardown add wall time.

| Run | Server CPU-seconds | Median user maximum ms | p99 of user maxima ms | Worst private ms | Worst fanout ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 6.26 | 2.067 | 14.629 | 18.194 | 33.060 |
| Candidate 1 | 6.42 | 2.964 | 14.016 | 18.081 | 33.054 |
| Candidate 2 | 6.52 | 4.186 | 17.038 | 21.485 | 34.338 |
| Control 2 | 6.66 | 6.219 | 16.386 | 19.797 | 36.968 |

The returning control is substantially slower than the first. Candidate CPU and many latency values lie between controls; private-tail rankings vary. The short reversed-order screen therefore does not isolate a gain or establish a causal regression. Fewer source-level lock calls alone do not justify adoption. No exact speedup claim is made. Eight private samples per user mean reported per-user p99 equals observed maximum; these are finite screens, not long-term percentile guarantees.

Every ordinary run passes 800,000 private observations, 1,048,576 fanout deliveries, 32/32 offered windows and 256 churn cycles. All three processes exit zero. All checked live resources retire, error/rejection/slow-consumer/pressure counters stay zero and routing copied bytes stay zero. The global channel remains by design. Run `python docs/benchmarks/2026-10-19-socket-status-screen/verify.py` to check the retained receipts, hashes and exact candidate-source boundary. compare.py retains the commands; repeating requires fresh output names. No additional long campaign is warranted for this candidate.
