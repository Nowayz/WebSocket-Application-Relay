# Coalesce retirement-queue wakeups

The selected change wakes a native worker only when publishing to an empty retirement FIFO. Publishing another job to an already active queue leaves its existing progress signal in place. This removes redundant waker operations without adding state, changing membership indexes, batching more jobs per turn, moving payloads, or relaxing queue admission.

Measured 2026-10-19. A release-build component burst changes from 1,001 worker-waker API calls to one, and its 1,000-source publication loop takes about 26% less elapsed time. This is a **publication-component improvement**, not a 26% whole-relay CPU, throughput or latency claim. Ordinary mixed results below establish no general speedup.

## Why the wake can be shared

Publication and the empty check use the existing retirement mutex. Progress retains the front job in the deque until it completes; an in-flight job therefore cannot be mistaken for an empty queue by another publisher. Pending retirement work keeps native polling nonblocking. The first publication into an empty queue still wakes its source worker, and refilling a drained queue wakes again. Shutdown signaling is unchanged. The production difference is an `is_empty()` check under that mutex and a conditional around the existing wake call.

A new regression uses a real `mio::Waker` and counts calls: two publications while work remains pending share one wake, complete with zero byte reservations, then a new publication after draining causes a second wake. Bounded progress loops prevent a stuck retirement flag from turning this test into a long spin. Existing checks retain FIFO, already-closing sources, observer delivery, late-join exclusion, recreated-room isolation, pressure fallback and resource release.

This does not change snapshot accounting or fix the deliberately undersized queue-budget case documented in the preceding retirement-cursor screen.

## Release component measurement

The diagnostic creates one pending oversized room retirement, then times dropping 1,000 prepared sources in distinct two-member rooms. All work remains queued during publication. The output uses a real Mio waker; its counted method calls `wake()`. Source/target setup, queue draining, FIFO/payload verification and cleanup are outside timing. No socket data I/O is modeled, and the diagnostic's counter adds work of its own. These are API-call counts, not an independent kernel syscall trace.

Each fresh process runs seven batches. Four processes run control/candidate/candidate/control, pinned to CPU 4, with release optimization and identical fixture source. The table reports the median batch time per process; every batch verifies exact notice IDs, ordering, zero remaining queue credit and a new wake after refilling.

| Process | Waker calls including initial publication | Median elapsed for 1,000 queued source drops |
| --- | ---: | ---: |
| Control first | 1,001 | 358.230 µs |
| Candidate first | 1 | 263.983 µs |
| Candidate repeat | 1 | 262.293 µs |
| Control last | 1,001 | 355.490 µs |

The component's release-test executable and atomic call counter are separate from the ordinary native executable; the native build contains no diagnostic counter. Raw seven-batch samples, Cargo test-artifact identities, source hashes and fixture source are retained.

## Ordinary 100k room-scope checks

Four fresh runs use the same frozen clients, disjoint CPU masks and individually pinned client workers. Each requests two seconds of mixed traffic: 100,000 capacity users, 8,192 additional room readers, window 4, 32 private pairs, two inspectors, eight churn workers at 32 cycles/second and eight offered fanout windows/second.

| Run | Server CPU-seconds | Verified sidecar private messages/s | Worst capacity-user RTT | Worst fanout RTT |
| --- | ---: | ---: | ---: | ---: |
| Control first | 8.93 | 1,169,978 | 5.586 ms | 55.215 ms |
| Candidate first | 9.34 | 1,073,042 | 4.485 ms | 65.039 ms |
| Candidate repeat | 9.29 | 1,066,306 | 4.684 ms | 79.519 ms |
| Control last | 9.29 | 1,076,005 | 5.753 ms | 79.470 ms |

The candidate's ordinary CPU samples are at the high end of this control range and do not establish a CPU reduction or exact equivalence. Its private rates are lower than the first control and close to the last. Observed capacity-user maxima are lower in these two candidate runs, while fanout maxima overlap. These few finite observations do not establish a general latency or throughput improvement; do not substitute the component result for these numbers. Listener socket placement is not held identical between fresh servers.

Every run meets all 16 offered windows and verifies 524,288 timed fanout deliveries, 512 churn cycles and 750,000 capacity departure notices in cleanup. Both clients and server exit 0. Final connections, sessions, queued bytes, variable bytes, read-buffer reservations and native arena slots are zero; routing copied bytes are zero. Batched room closes retain their exact departure sequence.

## Observer-scope and focused departure checks

A supplemental candidate observer-scope run passed, but its 21.053 ms worst capacity-user observation prompted a matching control. The control also passes and reaches 17.862 ms. Candidate/control fanout maxima are 37.828/34.562 ms; CPU is 9.34/9.40 seconds, and private rates are 1,064,704/1,329,836 messages/second. This single candidate-then-control pair demonstrates correctness in that existing scope, not an observer performance win or statistical equivalence. The preceding room-scope result must not be generalized to this mode.

Both observer cases complete 16/16 offered windows, all checked deliveries/churn/departures, normal exits and the same final zero-resource/copy gates. A further focused candidate test holds 100,000 recipients in one room and eight unrelated witnesses, verifies all 200,001 expected departure notices, observes worst witness RTTs of 1.391 and 5.571 ms during its two large departures, and exits normally. Those are client/kernel/loopback-inclusive round trips, not isolated relay-hop measurements or an SLA.

Selection is narrow: a repeatable reduction in real-waker publication cost with unchanged scheduling/ownership/admission and passing relevant end-to-end checks. No broad whole-relay speedup is established. The selected library passes 146 tests in 10.02 seconds, Clippy for library/binaries/tests with warnings denied, and formatting.

All steady traffic phases are two seconds. Full mixed commands take about 34 seconds because setup and exhaustive cleanup are outside measurement; the focused departure command takes about nine seconds. The component diagnostic takes hundredths of a second after compilation. Builds and diagnostics do not overlap ordinary timed traffic. No long soak or multi-hour test is used.

## Provenance and reproduction

Source parent: `fce50b9`; branch `codex/departure-quantum` in `/home/phil/.codex/worktrees/departure-quantum/war_next`. The original checkout's three dirty overlays and manually running services are unchanged.

Control executable: `/home/phil/dev/war_next/artifacts/candidates/departure-fifo-20261019-selected/war-relay`, SHA-256 `46f3765fa8a0ee879c33efd79648707fad81f118dd77ba543cf629ef95798dbd`. Selected executable: `/home/phil/dev/war_next/artifacts/candidates/retirement-wake-20261019-selected/war-relay`, SHA-256 `2c81c4113ebaf62bdf55e0a9ed03834bbe7f01c3911f0dd2d119b4a87324aba3`. `selected-build.json` records recompilation and exact final source hashes, including the committed regression. Measurements use the frozen `retirement-wake-20261019-final-candidate` executable; `selection.json` records its byte-identical selected copy. No runtime edit follows the ordinary measurements.

`run-native.py` and its receipts retain all ordinary commands; the observer control uses the same options with `--scope observers` and the control executable. The release component's four exact commands are in `component-runs.json`. Its ignored diagnostic test is archived as `component-census.rs`, with actual formatted test modules and input hashes for both copied Cargo projects. It is not installed in the selected test suite. Fresh output paths and serial timing preserve evidence.

For a short native repetition from the attached checkout:

```sh
taskset -c 8-15,24-31 python scripts/departure-fairness.py \
  --server /home/phil/dev/war_next/artifacts/candidates/retirement-wake-20261019-selected/war-relay \
  --audience 100000 --output /tmp/retirement-wake-fresh.json
```

Receipts are losslessly compressed. `analyze.py` recomputes the displayed comparison from retained samples and receipt statistics. Mixed receipts retain per-user CSV hashes and generator-derived aggregates; full CSVs remain at the original local artifact stems. The report does not independently recompute every user's aggregate. Base/selected source snapshots, the exact patch, build identities and validation are included. Run `python verify.py` here for offline checks; `--check-local` also checks current source, both native builds, the selected copy and both component executables. Verification launches no traffic.
