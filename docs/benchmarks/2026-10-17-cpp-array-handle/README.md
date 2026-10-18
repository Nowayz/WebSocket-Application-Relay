# Compact retained-body handle trial

**Decision: reject this refinement and restore the accepted modern C++ runtime.**

The earlier array prototype used a 24-byte transient `OwnedMessage` wrapper containing a shared pointer and length. This version uses a 16-byte shared pointer as the transient handle; only queued deliveries store the length. The delivery remains 40 bytes. It preserves one shared ownership allocation for retained bodies, one body copy, immutable publication, full size_t lengths, quotas, FIFO and the borrowed local path. It was tested from base commit `2513b3c`.

The hypothesis was that eliminating the redundant transient length would recover local-path performance while retaining the array allocation benefit. A native layout/ownership diagnostic verifies the size change and independence from later input mutation. The unchanged shared protocol corpus passes 776 cases / 2,311 exchanges.

## Controlled comparison

Twenty-four fresh eight-second trials compare accepted C++, the previous array prototype and this compact handle: four trials per build and topology, reversing build order on alternate repetitions. Four pinned workers, 32 disjoint private pairs, 256 connected peers, 1 KiB values, window16, the same frozen checked client. Exactly eight sources and eight receivers per worker. External TCP inode/fd/epoll observations verify every pair is either local or remote before and after measurement. CPU gates exclude setup, topology queries, templates, postchecks and retirement. The middle array arm remains in the middle; accepted/compact order reverses. No ordinary timing overlaps builds or profiling.

| Topology | Build | Median deliveries/s | Median CPU us/delivery | Per-trial p99 upper ms |
|---|---|---:|---:|---|
| local | control | 1,610,616 | 1.485 | 0.524, 1.049, 1.049, 1.049 |
| local | array | 1,609,567 | 1.485 | 1.049, 1.049, 1.049, 1.049 |
| local | handle | 1,620,109 | 1.483 | 1.049, 1.049, 1.049, 1.049 |
| remote | control | 1,344,854 | 1.800 | 2.097, 2.097, 2.097, 2.097 |
| remote | array | 1,515,356 | 1.632 | 1.049, 1.049, 1.049, 1.049 |
| remote | handle | 1,500,371 | 1.646 | 1.049, 1.049, 1.049, 1.049 |

The ordinary trials verify **295,498,880 deliveries**. Local CPU is essentially unchanged across all three arms. Remote compact-handle throughput is about 1.0% lower and CPU/delivery 0.85% higher than the earlier array prototype. This does not justify selection or the hypothesis that the transient wrapper caused the earlier local gap. The accepted/array local medians are also effectively equal in this cohort, reinforcing the need to keep small timing differences qualified. All local trials record zero retained-copy bytes; all remote trials exactly 1,037 copied bytes per verified delivery. Every case completes all 256 normal Close/EOF exchanges and returns descriptors and resource gauges to baseline. Histogram latency includes client scheduling/windowing, not an isolated relay hop.

No fresh 100,000-user or sanitizer campaign was run for this rejected refinement. Its failure to improve the already-rejected alternative did not justify promotion to those acceptance stages. Prior array population/mixed evidence and accepted sanitizer evidence remain separate.

## Separate baseline profile and next target

A fresh accepted-build mixed-load profile runs separately after ordinary trials: 512 authenticated global observers, one private pair, eight churn workers and two CBOR inspectors, six seconds of checked traffic. The profiler records cycles:u at 199 Hz with DWARF call stacks. The successful capture has about 3,000 samples and zero lost samples. It includes setup/retirement and does not measure off-CPU waits or isolated kernel cost.

| Function | Share of recorded event-cycle samples |
|---|---:|
| Worker::write | 24.75% |
| Worker::submit | 13.04% |
| Worker::drain | 11.18% |
| Worker::reconcile_buffer | 9.63% |
| pthread_mutex_lock | 3.48% |

Annotations concentrate samples on instructions immediately following locked updates to the retained-output budget and buffered-byte counters. Sampling skid prevents interpreting this as precise instruction timing. Some addresses remain unresolved/kernel-looking even in the cycles:u capture; the profile is not a complete causal accounting of wall time. The result motivates measuring global output-accounting cache-line placement and contention, not asserting a diagnosed false-sharing cause. Earlier zero-byte guards and per-worker-counter experiments remain rejected; any new accounting change must be isolated, preserve exact quota/retirement behavior, and demonstrate competing-traffic and population responsiveness.

The first attempted diagnostic lacked the server fixture auth password, so the checked client rejected authentication. That failed setup and driver are preserved explicitly and excluded from profile conclusions. The corrected successful capture is separate. Raw perf data stays local outside Git; build IDs, text report, instruction annotations, commands, exact binaries and client hashes are preserved here.

## Restoration and provenance

- Accepted executable restored: `b9dc20b92e7c09f2188d1780da8d691730112f442e60447de62ee6b98ae86da1`.
- Accepted static library restored: `237a499d0538c7ac3c46ce3af6c5fe3bc8c3e8c883cbdf96e591cbb99ecbb7d6`.
- Rejected compact-handle executable: `b6e840fcb3a259a17aecc902bb0499fa7f2df4053916ba9fc286fbd9ae35c4cf`.
- Restored build-source hashes are checked. The selected manual Rust executable/readiness and existing Rust experiment diff are unchanged. No push or deployment occurs.
- Complete compact-handle sources, patch and build metadata are under source/. Accepted and older-array sources are in the prior ownership study; the frozen client and controller provenance are in the controlled-placement study. This archive verifies that explicit dependency chain.
- Each case retains its ownership census, exact pairing plan, measured copy counts, gates, resource counters and complete client logs. Large JSON/text originals have separate uncompressed hashes.

Verify with `python3 docs/benchmarks/2026-10-17-cpp-array-handle/verify.py`.
