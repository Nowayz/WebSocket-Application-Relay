# Native socket API census with traffic-phase separation

Diagnostic only: retain the production runtime. A copied native build counts socket read/send attempts, poll calls, output notification wakes and writable-interest reregistration outcomes. No production telemetry, setting, queue, protocol or writer change is selected.

The preceding [CPU split](../2026-10-19-process-cpu-accounting/README.md) charged about 81% of process CPU to kernel time. This host rejects a cycles:k probe with perf_event_paranoid=2. The task leaves that policy unchanged and uses source-level counters instead. They measure API submissions and outcomes, not sampled kernel cycles, exact syscall cost, stack attribution or blocking-time duration.

## Instrumentation and scope

Four fixed I/O rows count native codec reads, codec control writes, socket sends and vectored socket sends. Rows retain attempt/success/WouldBlock/Interrupted/error counts, requested/returned bytes, zero/short results and maxima. Counts are made around the existing result and the identical result returns to the caller. Separate rows count worker poll, writable-interest reregister and output-notification wake calls. Initial socket registration, deregistration, listener accept, administrative HTTP I/O and stop-only wake calls are outside these rows; it is not a complete kernel syscall trace.

Each caller thread owns fixed-size TLS counters. A diagnostic-only phase index uses one acquire load per counted operation and release stores at quiescent phase transitions. There is no shared counter RMW, per-call allocation, clock read or body copy. Instrumentation and changed code layout/lock duration can perturb the workload, so its latency/CPU/throughput are excluded from ordinary performance comparisons. JSON is emitted on worker exit.

The initial lifetime-only census passed, but included a large teardown notification load. The accepted follow-up adds a diagnostic-only administrative phase marker in the copied source and sets it to measurement after the ready correctness gate, before client continuation; cleanup begins after the measured correctness gate, before continuation. Setup, measurement and cleanup each retain separate fixed counters. This route and phase state exist only in the copied diagnostic, not the current runtime or manually running services. Raw receipts explicitly mark diagnostic_only=true; their existing profile_diagnostic flag describes only perf/heap runner modes.

## Phase-separated measurement counts

Two fresh processes offer two traffic seconds each, plus setup/verified cleanup. Private uses 64 pairs; fanout uses 2,048 readers and four-message windows. Both use 1 KiB byte-string values, four pinned native workers, eight pinned client workers on separate cores, no inspectors/churn/heartbeat/rate policy and unchanged admission/memory bounds. The client and its source attribution are fixed in receipts. This is workload coverage, not an implementation comparison or population fairness acceptance.

| Measurement phase | Private | Fanout |
| --- | ---: | ---: |
| Delivered application frames | 4,646,768 | 3,514,368 |
| Vectored send attempts | 4,646,768 | 3,509,401 |
| Read attempts | 2,130,852 | 1,831 |
| Read WouldBlock results | 290,522 | 429 |
| Poll calls | 20,736 | 35,917 |
| Output-notification wakes | 0 | 28,826 |
| Writable-interest reregisters | 0 | 0 |
| Send WouldBlock / short results | 0 / 0 | 0 / 0 |

All sends completed fully. Private has exactly one vectored-send attempt per delivery; fanout has 0.99859 attempts per delivery, so opportunistic batching saves few calls in this specific measured mixture. Private positive reads average about 2,639 bytes, and fanout publisher reads about 1,279 bytes. These counts motivate examining send density and receive behavior, but do not prove which operations consume the measured kernel CPU. Counts do not imply equal cost. Zero reregisters gives no basis for optimizing writable-interest toggling here; zero short/blocked sends gives no basis for optimizing those outcomes from this workload.

## Independent accounting checks

All four worker reports reconcile attempts with successes/errors. During measurement, cumulative successful data-send bytes equal the independent delivered CBOR-byte counter delta plus exactly four WebSocket header bytes per delivery. Successful read bytes equal the incoming CBOR-byte delta plus exactly eight masked header bytes per incoming message. Both fixtures use complete 1 KiB-value frames; the exact 4/8-byte reconciliation also confirms that setup/control/teardown bytes did not contaminate this phase.

Both phased runs pass complete sender/body/FIFO, exact census/counter, zero routing-copy and six-resource-gauge retirement gates and normal client/server exits. The private empty-worker gate retains 192 small arena slots under the existing warm-arena policy; the fanout gate has zero. This is not an RSS or leak claim. Lifetime-only runs also pass; their counts remain archived as preliminary whole-process observations, not substitutes for the accepted traffic-phase table.

Fanout cleanup produces 2,098,176 departure deliveries: 2048*2049/2, matching the notification contract as readers leave while peers/publisher remain. That lifecycle work explains why a lifetime census includes many tiny packets. It must not be combined with the earlier measurement-gate CPU split as if both covered the same interval. The earlier [queued-write census](../2026-10-19-queued-write-turns/README.md) already labels its process-lifetime scope; this follow-up clarifies the difference.

The next useful fairness check is a bounded large-room departure burst against unrelated healthy users. The source removal path synchronously walks remaining channel members under the registry write guard; count evidence alone does not establish its user-visible delay. No departure policy, lifetime ordering or batching change is adopted here.

## Provenance and reproduction

Base: 3e49c54. Clean copied snapshots exclude the three pre-existing dirty source overlays. The lifetime build changes only server.rs/socket.rs; the accepted phase build additionally adds its marker to admin.rs. Complete source hashes and frozen binary identities are in build receipts. The accepted executable is artifacts/candidates/io-census-20261019-phased-v2/war-relay. An initial phase build failed for an unqualified Ordering::Release name; the corrected v2 was freshly compiled, not reused from that failed build. All existing services, convenience executables and host policies were preserved.

Raw receipts include thread counters and all gates. Compressed original/diagnostic source, patches, copied phase-aware runner, exact driver and analysis script are archived. The driver sets PYTHONPATH to the repository scripts directory; it uses original artifact names and must be given fresh paths before replaying. Build with scripts/build-snapshot.py, keep all runs/builds/checks serial, and adapt CPU masks to another host. There are eight diagnostic traffic seconds across lifetime and phased cases; no long campaign was used.

Run `python docs/benchmarks/2026-10-19-native-io-census/verify.py` to audit saved rows, phase markers, byte conservation, build identities and final resources without starting services. The phase runner patch is deliberately unselected; do not expose its marker as a production endpoint.
