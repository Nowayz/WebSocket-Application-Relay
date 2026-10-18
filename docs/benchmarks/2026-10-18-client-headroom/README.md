# Client headroom and backlog checks — 2026-10-18

## What changed

Keep a small reporting fix in `scripts/mixed-capacity.py`, code commit `c83c99c`: expose whether a private send fell more than one configured message period behind its intended time. No runtime, allocator, routing, wire, ownership, FIFO or server policy change is adopted. Historical defaults and frozen benchmark clients remain unchanged.

The two proposed client optimizations are rejected. More frequent I/O polling does not consistently improve tails and consumes more CPU. Halving the client worker count overloads the generator despite successful eventual delivery. A separate controlled private-only experiment demonstrates that physical CPU headroom changes the measured result materially, without changing relay code.

## Eleven short population cases

All eleven accepted setups use 100,000 connected private users, 16-user rooms, 1 KiB values, four probes/user/second and eight-second offered phases: **35.2 million measured private probes**. Every user contributes 32 observations; its nearest-rank p99 therefore equals its maximum. These screens cannot establish strong per-user p99 guarantees or long-term stability. An overloaded generator extends actual completion past the offered eight seconds; the original elapsed times remain recorded.

Seven cases also include 8,192 fanout recipients, four frames/window at a nominal 16 Hz (123–128 of 128 planned windows complete), eight churn workers at 4 Hz and two inspectors. The relay has eight individually pinned physical workers on CPUs 0–7; capacity uses 8–11,24–27 (eight logical CPUs sharing four physical cores), and inspection uses 12–15,28–31. The groups share no physical cores. Diagnostic stage clocks/storage are enabled equally in these cases. Compare cases only within their stated workload. Every submitted fanout matrix is verified, but completed work must not be confused with the nominal schedule:

| Case | Completed / nominal fanout windows |
|---|---:|
| Event control first / second | 128 / 128; 127 / 128 |
| Interval-8 first / second | 128 / 128; 127 / 128 |
| Four-worker first / second | 128 / 128; 128 / 128 |
| Eight-worker return control | 123 / 128 |

The receipt field `fanout_windows.offered` is the nominal `seconds × target Hz`, not proof that the generator submitted every planned window. Preserve these shortfalls. Event comparisons pair equal completed window counts, but the worker-count return control completes fewer fanout windows. These are pacing/latency diagnostics, not a fixed delivered-bandwidth comparison.

### Rejected: poll external events every eight tasks

Pinned Tokio 1.53.3 normally polls external events after 61 task polls. Its installed official builder/scheduler source is included. A smaller interval is a plausible way to improve event responsiveness, but incurs more synchronization/syscalls. The candidate changes just one runtime-builder line to `.event_interval(8)`. Both executables are compiled sequentially in the same clean project directory with the same compiler, Cargo.lock and release profile. Four fresh processes run control/candidate/candidate/control; no builds/profiles overlap measured phases.

| Configuration | Client CPU-seconds, repeats | Maximum intended-to-receive latency, repeats |
|---|---:|---:|
| Default interval 61 | 30.29; 32.19 | 11.809; 8.946 ms |
| Interval 8 | 32.63; 33.00 | 22.469; 13.267 ms |

The candidate's medians improve slightly, but CPU is about 5% higher on the mean of the two repetitions and its tails do not meet the responsiveness objective. One candidate run has one-second post-send p99 excursions of 11.09 and 13.67 ms. Reject the change; no new event-interval knob is shipped. This does not prove that scheduler event frequency never matters in other workloads.

### Rejected: four capacity workers

Two runs use four capacity workers with the identical default-61 executable and identical allowed CPU mask; the sidecar remains at eight workers. A fresh eight-worker control follows. The preceding control is also retained. The only intervention is the capacity worker count.

| Capacity workers | Client CPU-seconds | Maximum pre-send delay | Maximum intended-to-receive latency |
|---|---:|---:|---:|
| 4, first run | 26.61 | 1,994.214 ms | 2,244.144 ms |
| 4, second run | 26.14 | 2,026.064 ms | 2,275.991 ms |
| 8, return control | 32.34 | 7.008 ms | 10.361 ms |

Every body/FIFO/count/resource check passes, but four workers fail to sustain the requested pacing. Their smaller CPU total is not a useful efficiency win for a realtime workload. Reject this configuration. It is evidence that the measured outcome depends on generator scheduling/capacity; it does not assign every interval exclusively to a particular process or kernel stage.

### Kept guidance: give the generator physical CPU headroom

Four separate **private-only** cases remove the fanout/churn/inspection sidecar in both configurations, freeing CPU cores for an isolated placement experiment. Keep the same eight native relay workers, same eight client workers, executable hashes, socket settings, payloads, population and 4 Hz workload. Pin each I/O thread individually and verify its affinity at gates/monitoring. The client is placed either on eight logical CPUs sharing four physical cores (`8–11,24–27`), or eight distinct physical cores (`8–15`); the relay remains on `0–7`. Fresh processes run SMT/physical/physical/SMT. No physical core is shared between relay and client.

| Client placement | Client CPU-seconds, repeats | p99 across users of their worst send-relative latency | p99 across users of their worst intended latency |
|---|---:|---:|---:|
| Eight workers on four physical cores with SMT | 25.71; 25.78 | 1.841; 1.776 ms | 4.040; 3.890 ms |
| Eight workers on eight physical cores | 19.64; 19.38 | 1.647; 1.546 ms | 3.555; 3.791 ms |

Mean client CPU falls **24.2%** for the same checked 3.2 million private probes per run. The cross-user p99-of-worst values improve in these repetitions. **The absolute worst intended latency does not improve consistently:** physical runs reach 6.762 and 5.761 ms versus SMT 6.587 and 6.429 ms. Retain that excursion. This is an end-to-end configuration result, not a relay-code speedup or proof that all post-send delay is client scheduling. Do not compare these private-only figures directly with the mixed figures, or assume the gain transfers unchanged to fanout.

The host has 16 physical cores. The mixed test already assigns eight to the relay and four each to capacity and inspection; giving capacity eight physical cores would require changing that mixed allocation or using another generator host. No CPU overlap or hidden reallocation is introduced into the mixed comparisons. Existing `scripts/capacity.py --pin-client-workers --client-cpus ...` supports the tested private placement; no new worker-count/affinity feature or changed default is needed.

## Kept backlog signal and verification

The mixed runner reads the existing companion `.windows.json` **after measurement**, verifies all private cohort sample counts, hashes the timeline and adds:

```json
{"generator_lag":{"available":true,"period_ms":250.0,
 "maximum_pre_send_ms":2026.064163,
 "windows_over_one_period":[1,2,3,4,5,6,7],"exceeded_one_period":true}}
```

The 250 ms threshold comes from the requested 4 Hz workload, rather than a new arbitrary latency SLO. The test detects a sender that fell behind its own cadence by more than an entire period. `exceeded_one_period: false` does **not** certify low latency, precise pacing or spare CPU headroom. Pre-send includes wakeup, preparation and earlier room sends; a previous slow response can also delay subsequent sends. The field reports the observation without inventing a root cause.

The final console result exposes `generator_exceeded_one_period` alongside the existing functional `passed` field. Delivery/counter success remains distinct from pacing quality, and overloaded trials remain available for diagnosis. An older client that emits only the CSV remains supported: lag is explicitly unavailable, not assumed zero. The existing explicit stage option still requires its timeline.

Replay against all seven real mixed timelines correctly identifies both four-worker overloads and none of the five faster cases. A **two-second, 128-private-user / 64-fanout-reader** end-to-end smoke through the final runner verifies the new JSON/hash/console fields, existing stage checks, full counters and cleanup. This smoke is integration evidence, not a performance comparison. Python compilation and whitespace checks pass. No Rust source was changed, so no additional library test run is claimed.

## Failed setups and evidence limits

The first event-control artifact lacked executable permission and stopped with status 126 before connecting; executable mode is corrected and the failed receipt is retained separately. No timed phase occurred.

The first private-placement setup stops before traffic because host TCP census reports 100,001 established sockets while application metrics report 100,000. Its exact source/receipt/log are retained. The cause was **not reproduced or established**; no assertion is relaxed. A diagnostic copy of the existing capacity runner adds socket-inode ownership/duplicate/foreign-row observations while retaining the original raw-row assertion. All four completed placement runs have exactly 100,000 raw rows, unique inodes and server-owned inodes at connected/ready/measured/postcheck, no duplicate/foreign matches, and zero at retirement. The original production helper is unchanged. Do not describe the unexplained setup failure as fixed.

The machine is a desktop host, not a controlled lab. Short-run tail excursions and both repetitions remain in the record. Every accepted case has the frozen relay `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`. Controls and worker/placement cases use clean capacity executable `c74a950efc932b5c7dfe8346e5ebfaf4d3d82a80b68d5b6a0e5bd4c9e655e906`; interval-8 uses `2993e0693862be9a027309cc348ca44a2d1a079eedfe8641323cb7b6c8f9727d`. Client source base is clean `5c95be5`; the three user-modified relay/C++ files are excluded and remain byte-identical. Native, bridge and static manual services are not restarted.

## Reproduction

`manifest.json` protects this archive, compressed CSVs/binaries and decoded CSV hashes. `dependencies.json` binds preceding source/vendor/binary provenance. The exact build script, campaign commands, runners, raw receipts, analysis and validation sources are included. Copy saved artifact scripts back under repository `artifacts/` before replaying commands that derive the root from their parent. CPU sets are specific to the recorded topology; verify physical siblings before adapting them. Decompressed executable snapshots require executable mode before running.

Run `python3 docs/benchmarks/2026-10-18-client-headroom/verify.py` to verify all evidence, complete user matrices, exact quantiles, seven mixed workloads/fanout totals/stage witnesses, four private ownership censuses/affinities/counter receipts, backlog classification and the final smoke. No C++ comparison, saturation-throughput improvement, production performance change, memory reduction, or long-soak reliability result is claimed.
