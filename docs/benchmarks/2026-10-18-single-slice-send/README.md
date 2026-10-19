# Single-slice socket send: rejected short screen

Decision: retain the existing vectored writer. A candidate uses ordinary `write`/socket `send` when only the inline header or remaining body is present; header-plus-body writes keep vectored I/O. It does not establish a satisfactory whole-relay result across short churn-heavy and ordinary-private workloads. No native code, configuration or executable was selected.

A separate two-second private-forwarding profile first checks whether byte-value validation is worth specializing. In the selected native binary, the general validator initializes 512 bytes of work-stack storage at entry. However, its self sample share is only 0.50%, versus 21.42% for bounded receive/routing and 20.37% for socket submission. A stack initialization alone is not evidence of a meaningful whole-relay cost. Prior rejected leaf/direct-byte experiments already show that parser-only gains need not improve forwarding. No new parser path or unsafe uninitialized storage was introduced.

The output experiment recognizes that inline welcome/control frames have no body, and partial writes can leave only a body. It calls the already existing SocketWriter::write path for those cases, preserving Mio try_io, checked slice lengths, Interrupted/WouldBlock behavior, write quantum and offset accounting. Ordinary two-slice frames still borrow the original body and use send_vectored. This is a hypothesis about syscall/setup overhead, not a claim that socket submission's entire profile share is removable.

Eight fresh ordinary processes measure two seconds each. Each workload uses control/candidate/candidate/control order; four native workers are pinned to CPUs 0-3, eight identical client workers to CPUs 4-7,20-23. Churn-heavy traffic has eight join/self-echo/Close workers and one active private pair. Ordinary forwarding has 64 private pairs and no churn. All private values contain 1 KiB. Inspection, fanout and extra stable singleton rooms are disabled; existing census gates remain enabled. Builds, tests and profiling do not overlap ordinary timing.

| Workload | Variant | Native CPU-s / million checked operations | Private maximum (ms) | Churn lifecycle maximum (ms) |
| --- | --- | ---: | ---: | ---: |
| churn-mixed | control | 3.3530 | 1.480 | 2.796 |
| churn-mixed | candidate | 3.3683 | 0.461 | 2.511 |
| churn-mixed | candidate | 3.3826 | 1.184 | 2.246 |
| churn-mixed | control | 3.2805 | 3.018 | 4.123 |
| private | control | 1.1374 | 1.893 | 0.000 |
| private | candidate | 1.1275 | 3.820 | 0.000 |
| private | candidate | 1.1349 | 2.718 | 0.000 |
| private | control | 1.1446 | 1.333 | 0.000 |

A checked operation in the churn-heavy denominator is a private delivery or a complete checked churn lifecycle; those have different costs, so this unpaced mixed rate is not a fixed-mix service-cost metric. Exact delivery, join and byte counts remain in the raw receipts. Both churn-heavy candidate CPU costs exceed both controls, although candidate churn maxima improve. Both ordinary-private candidate maxima exceed both controls, while CPU cost is slightly lower. This tradeoff does not justify replacing the simple unified writer under the responsiveness goal. These finite maxima do not prove a kernel cause or universal regression.

The first attempted churn-only fixture (zero private pairs and no fanout) is rejected by the unchanged client's workload bounds before timing. Its failed receipt is retained; the valid churn-heavy fixture adds one private pair without weakening any check. Profiling is diagnostic and excluded from ordinary comparisons. The full perf recording remains local at artifacts/byte-validation-profile.perf.data; report/annotation excerpts and its receipt are archived.

All eight valid trials pass payload/FIFO/exact counters, zero routing-copy counters, normal process exits and final resource retirement. All 92 clean candidate library tests pass in 0.50 seconds, including short-write/body-ownership and shutdown tests. Clippy passes all targets with warnings denied. The candidate/control source manifests differ only in src/server/socket.rs, against clean base d5f35d1. Protected dirty overlays, manual services and convenience binaries are preserved. No full-population retest is needed for an unselected small-screen candidate.

Run `python docs/benchmarks/2026-10-18-single-slice-send/verify.py` to recompute ordinary results and audit identities, validation and the separate diagnostic. The complete candidate patch is retained for review without changing production.
