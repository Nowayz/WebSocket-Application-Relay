# Reject timer-slot scheduling shortcut

Decision: retain the selected runtime unchanged. The candidate removes a redundant token-map lookup during timer expiry scheduling. It improves its component measurement by about 22%, but the short ordinary 100,000-user comparison gives no reason to adopt it for the latency-first relay: both candidates use slightly more native CPU and have higher worst fanout latency than either control. These four runs do not prove a universal regression or its cause. The source experiment and tests are archived rather than selected.

## Candidate and component scope

The timer heap already supplies a connection slot. The current schedule_timers reads that connection's stable token, then schedule(token) hashes it back into a slot. The candidate extracts the existing validation and queueing body into schedule_slot(slot, token). Ordinary readiness still resolves tokens through the canonical map; timer discovery supplies its known slot directly. Slot/token validation, output-only wake upgrades, duplicate-ready suppression, 1,024 expiry discovery bound, deadlines, FIFO, shared bodies and memory budgets remain unchanged. It adds no field, setting, dependency or persistent allocation.

Clean source at 3f65efd excludes the three pre-existing dirty overlays. Two copied release library-test executables include an identical measurement helper. It establishes 1,024 actual pending TCP connections on one worker, then prepares all timers due and resets flags outside each measured interval. Seven batches of 100 iterations time schedule_timers only. Each iteration checks exactly 1,024 ready entries in token order, an empty timer heap, scheduled=true and output_only=false. Timing includes timer removal/heap maintenance and scheduling; it excludes timer setup, connections, checks, decoding, forwarding and socket traffic. Whole-process affinity is CPU 0. This synthetic due batch is not packet latency or an expiry-flood service benchmark.

Four fresh processes run control/candidate/candidate/control, serially, with release builds completed beforehand. Each median is across seven per-batch ns/timer values:

| Trial | Median ns per scheduled timer |
| --- | ---: |
| control-1 | 32.3955 |
| candidate-1 | 25.2551 |
| candidate-2 | 24.9664 |
| control-2 | 32.0200 |

The paired reductions are about 22%. A preliminary control invocation completed but the driver's parser did not handle the test harness prefix on the JSON line. That parser failure is preserved in initial-parser.log and excluded from the accepted four-process comparison. The corrected driver reads the marker wherever it occurs on the line. No binary change was needed.

## Ordinary mixed workload

Four fresh native processes use the current inspection/capacity clients, 100,000 private users at 4 Hz, 8,192 fanout readers, four-message windows offered at 16 windows/s, eight lifecycle workers at four cycles/s and two protocol inspectors. Eight native workers are pinned to separate cores in CPU 0–7; capacity and sidecar processes use their existing disjoint CPU masks. Client threads are not additionally individually pinned. Each measurement offers two traffic seconds; admission, settling, exact output and verified retirement add wall time. Native CPU spans the runner's gates including settling, not exactly two seconds. No builds, tests, profiles or analysis overlap the ordinary runs. Full commands and hashes are in each receipt.

| Trial | Native CPU seconds | Worst private ms | Worst intended private ms | Worst fanout ms | Worst intended fanout ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| control-1 | 6.65 | 6.103474 | 8.073118 | 54.404786 | 55.384690 |
| candidate-1 | 6.82 | 6.454975 | 7.919089 | 57.188631 | 58.412557 |
| candidate-2 | 6.79 | 5.853211 | 7.920126 | 63.549337 | 64.185320 |
| control-2 | 6.76 | 4.874428 | 8.602139 | 56.220916 | 57.356324 |

Both candidate CPU values exceed both controls, as do their fanout maxima. Private observed maxima overlap; intended private maxima are slightly lower in the candidates. The component gain does not establish a full-relay benefit. The ordinary workload disables heartbeat/idle timers and is not intended to stress sustained timer expiry; it checks the candidate's resulting native build under the core mixed workload. Compiler layout, scheduling or run variability may contribute, but these measurements do not isolate their cause. Avoid adopting a cold-path shortcut on the strength of its microbenchmark while the latency-first workload lacks supporting evidence.

Every run passes all 32 offered windows, payload/sender/FIFO checks, exact counters, zero routing copied bytes, restored descriptor baseline, all checked final resource gauges zero, retained root channel and exit_codes=[0,0,0]. Each private user contributes eight observations (800,000/run), so nearest-rank per-user p99 is its maximum. This is finite-sample evidence, not a long-term p99 or latency guarantee. Fanout has 128 observations per reader (1,048,576 deliveries/run). Exact user CSVs and timelines remain at the original artifact paths, with hashes in receipts; they are not duplicated here.

The candidate passes 134 library tests in 10.04 seconds, strict library/test Clippy in 2.05 seconds, and formatting. The extra regression checks timer expiry both when a connection is unscheduled and when it already has an output-only ready entry: exactly one entry remains and full input/timer processing is enabled. The selected source returns to its original 133-test suite; this candidate-only test and the measurement helper are archived with the rejected experiment. No runtime service, convenience executable or unrelated dirty source file changed.

## Evidence and reproduction

The report contains component and native build receipts, compressed source, candidate/measurement/test patches, all ordinary receipts, accepted component logs, drivers, freeze helper and test/check logs. Native control and candidate source maps differ only in src/server.rs and cfg(test) fairness tests. The frozen native candidate is artifacts/candidates/timer-slot-20261019-native/war-relay; it remains unselected. The control is artifacts/candidates/metrics-static-20261019-native-candidate/war-relay.

Run `python docs/benchmarks/2026-10-19-timer-slot-screen/verify.py` for an offline audit. Add `--check-local-csv` from the repository root to verify original private/fanout CSV hashes and audience counts. Drivers record exact executable paths and choose artifact output names that must be fresh before reuse. Reconstruct the component sources from the base plus measurement.patch, with candidate.patch added only to the candidate. The native candidate uses candidate-test.patch instead of the measurement helper. Freeze builds serially and check fresh:false, source and binary identities before running; adapt CPU masks to another host.

This screen used eight ordinary traffic seconds and sub-second component runs, plus connection setup, verification, compilation and the existing ten-second bridge timeout regression in the library suite. No longer campaign is justified for this rejected candidate.
