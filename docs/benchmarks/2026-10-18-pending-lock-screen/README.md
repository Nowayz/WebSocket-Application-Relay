# Notification-queue lock investigation

Leave the current native worker loop selected. A reusable local notification queue demonstrably reduces failed attempts to acquire the pending-queue mutex, but four short uninstrumented mixed-load runs do not establish a repeatable whole-workload latency or CPU benefit. No runtime code or running service change is adopted.

## Mechanism and diagnostic

The current worker holds its signal's pending mutex while translating tokens into connection slots and scheduling them. Senders use that mutex to append notifications. The prototype swaps pending tokens into a second reusable `VecDeque`, releases the mutex, then performs the same token/connection checks and scheduling outside the lock. The second queue is reused, with no deliberate allocation per turn. It adds retained queue capacity; payload ownership, per-source FIFO, notification coalescing, admission, protocol limits and output budgets are unchanged.

A separate diagnostic build replaces only the sender's mutex acquisition with `try_lock().unwrap_or_else(...)` and counts failed attempts before taking the ordinary lock. It exports one temporary metric. A temporary controller records that counter from the common ready gate until the private client reports measurement complete. These are attempted lock collisions, not nanoseconds waiting, a count of slept threads, or a production contention rate. Instrumentation alters behavior/layout and can perturb timing; its latency/CPU results are preserved but excluded from the performance comparison.

| Diagnostic build | Failed pending-lock attempts in the gate |
| --- | ---: |
| Current scheduling under the lock | 49,441 |
| Reusable queue handoff | 258 |

Both gates begin at zero, and both diagnostic cases complete all 96 offered fanout windows. The observed collision reduction is about 99.5%, from one matched diagnostic run per implementation. This demonstrates the mechanism, not a statistically established application improvement or proof that this mutex dominates connection-driving time.

## Uninstrumented six-second comparison

Fresh processes run control/handoff/handoff/control. Each has 100,000 private users in 16-member rooms, 1 KiB values at four probes/user/second, 8,192 ordinary-room fanout recipients, four messages/window at 16 Hz, eight churn workers at 4 Hz and two inspectors. Every traffic phase is six seconds; setup and exhaustive checked departures/Close cleanup are outside timing. Private users contribute 24 observations each. Each fanout recipient contributes 384 observations. All runs complete exactly 96/96 offered windows.

Server workers are individually pinned to eight physical cores 0–7. Private and sidecar clients use 8–11,24–27 and 12–15,28–31, respectively, with eight logical workers on four separate physical cores each. No client shares a physical core with the server. Exact client stage clocks are enabled equally in the four comparison cases. The clients remain below their own 250 ms pacing period; this is not a workload-independent latency SLO. Binary/runner hashes and topology are recorded.

| Trial | Variant | Median private worst, ms | Population p99 of private worst, ms | Worst private user, ms | Worst fanout window, ms | Server CPU-s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Control | 1.456 | 5.678 | 9.239 | 81.102 | 20.71 |
| 2 | Handoff | 1.495 | 6.376 | 10.194 | 61.952 | 20.96 |
| 3 | Handoff | 1.629 | 6.592 | 10.052 | 74.435 | 20.89 |
| 4 | Control | 1.517 | 20.629 | 22.858 | 71.704 | 21.04 |

The control's returning tail is much worse than its first run. Candidate private tails fall within that observed range; CPU totals overlap closely, and fanout maxima vary. This is inconclusive evidence of an application benefit, not proof that the candidate worsens latency. Retain the simpler current implementation and avoid treating a lock-collision reduction as an automatic user-facing speedup.

The returning control's largest delays occur after socket-send completion in the client stage clocks, rather than in the pre-send stages. That interval includes client receive scheduling, kernel and relay time, so it cannot assign the spikes exclusively to the server or mutex. Stage percentiles must not be added together. With 24 samples/user, nearest-rank per-user p99 equals each user's observed maximum; these finite screens cannot establish long-term per-user p99 or deadline guarantees. Fanout timing starts at the beginning of a four-message window and includes client windowing/verification.

## Correctness and scope

All six cases, including both diagnostic cases, pass exact body/sender/FIFO, audience matrix, counter, normal Close/departure and final resource checks. The four uninstrumented runs verify **9.6 million timed private probes and 12,582,912 timed fanout deliveries**; the two diagnostics add **4.8 million private probes and 6,291,456 fanout deliveries**, separate from setup/postcheck/departure receipts. Connections, sessions, queued bytes, variable bytes, output-queue capacity and read-buffer reservations return to zero; the global channel remains. Routing copied bytes and error/pressure/rejection/timeout counters remain zero, descriptors retire, and all child processes exit successfully.

This study establishes real pending-lock contention and a way to reduce collisions. It does not establish that the handoff improves response latency, total CPU, bandwidth, 100,000-member broadcasts, long-duration reliability, or WAN/TLS behavior. The candidate is left unselected before broader pressure and unit validation; finite healthy traffic correctness is not exhaustive race verification. The original runtime and all three pre-existing user overlays remain unchanged.

## Reproduce and verify

`python3 docs/benchmarks/2026-10-18-pending-lock-screen/verify.py` verifies archive hashes, decoded sample/identity/quantile and protocol-counter checks, resource retirement, diagnostic gate differences and patch application against the pinned Git revision. Raw JSON/CSV/timeline files are losslessly gzip compressed; decoded CSV and timeline hashes match the original receipts.

`summary.json` pins base revision `ada100d`, build paths and derived values. Both ordinary binaries are sequential clean release builds in the same project and Cargo target paths. Source extraction excludes user overlays. Only `source/handoff.patch` differs in the uninstrumented candidate. The two diagnostic builds additionally apply the identical `source/diagnostic.patch`; they use the archived temporary controller, whose only additional observation is the post-private-measurement metrics snapshot. No builds or profiles overlap any measured phase. Compilations during diagnostic cleanup occur after the measurement gate and are excluded from its timing interpretation.

For reproduction, extract `git archive ada100d Cargo.toml Cargo.lock src examples vendor`, build `cargo build --release --bin war-relay`, freeze the control binary, apply `source/handoff.patch`, and rebuild sequentially in the same paths. Use fresh output names with the exact commands in the receipts through `scripts/mixed-capacity.py`: `--users 100000 --seconds 6 --readers 8192 --scope room --pairs 0 --churn-cycles-per-second 4 --fanout-windows-per-second 16 --exact-fanout-latency --latency-stages`, and the pinned capacity client. Diagnostics omit the two optional timing/output flags equally. Inspect topology before adapting the host-specific CPU masks. Never substitute instrumented timings for the ordinary comparison.
