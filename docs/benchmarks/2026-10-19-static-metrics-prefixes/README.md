# Compile static metrics exposition prefixes — 2026-10-19

Selected: assemble each immutable HELP/TYPE/name prefix at compile time, copy it into the existing response String, and format only its current numeric value. Previously the renderer passed the same static names, help strings and types through general formatting on every request. A private sample! macro produces the static-prefix/value tuple; the same three arrays retain their source sampling groups. All 43 metric definitions, types, HELP text and value expressions are unchanged. Metrics fields and update paths are untouched, no values are cached, and no lock, timer, public API, runtime setting or dependency is added.

This is a data-representation change in the public Metrics::render operation. It preserves allocation/construction, all atomic loads, numeric formatting and destruction in the measurement. It is not a relay throughput or end-to-end HTTP latency claim.

## Ordinary rendering comparison

Two independently compiled frozen helpers use identical fixtures and control/candidate/candidate/control order on CPU 8. Each process/case records seven batches of 20,000 renders, with an opaque self pointer, arguments and returned String. Zero counters use 100,000 sessions/10,000 channels; max assigns every exposed atomic and the two usize arguments their maximum, with draining true; budget assigns distinct nonzero counters and a live decoder-budget lease plus a rejected reservation. Each initial rendered output is saved outside timing. All twelve files are byte-identical within their corresponding fixture, including complete names, HELP/TYPE metadata, ordering and numeric values. No profiler/build/test overlaps timing.

| Fixture | Control process medians ns/render | Candidate process medians ns/render | Reduction from means of process medians |
| --- | ---: | ---: | ---: |
| Zero counters | 2,230.5 / 1,977.8 | 636.4 / 624.3 | about 70% |
| Maximum-width values | 2,408.7 / 2,191.9 | 848.2 / 831.7 | about 64% |
| Active read budget/nonzero counters | 2,134.6 / 1,981.5 | 624.5 / 614.4 | about 70% |

There is some control-run drift, but both candidate medians are well below both controls for every fixture. These are finite per-process component results on the recorded host, not language rankings or production latency guarantees.

## Allocation and footprint tradeoff

Separate tcmalloc diagnostics compare zero and 64 renders, subtracting startup/fixture work. Both implementations have one 8,192-byte allocation per common zero/budget render, and two allocation/reallocation events totalling 24,576 requested bytes per max render. Argument-length differences contribute 1/64 byte to reported totals; no heap improvement is claimed. Each profile header is checked against its complete stack rows. Diagnostic timings/RSS are excluded.

The native executable's .rodata grows from 170,920 to 173,760 bytes (+2,840), while .text shrinks from 2,378,581 to 2,377,701 bytes (-880). These named-section measurements are not a total-file or RSS claim. The helper's separate section measurements are retained too. The read-only footprint is the cost of avoiding repeated metadata composition; it adds no per-connection storage or runtime initialization.

The existing 8 KiB reserve remains a hint. Current fixture lengths/capacities are zero 7,557/8,192, budget 7,632/8,192 and max 8,270/16,384 bytes. The old source comment incorrectly claimed every twenty-digit exposition fits within 8 KiB; the current format's loose all-twenty-digit bound is 8,365 bytes. The comment now states that wider values or future fields can grow normally. No numeric limit or capacity policy changes.

## Native HTTP and regression checks

Fresh frozen native processes receive authenticated metrics requests in ready, draining and resumed states. Both produce byte-identical complete bodies, status 200 and unchanged text/plain Prometheus content type. Both drain/resume operations return 204; readiness switches 200/503/200; all checked live resource gauges stay zero. Every process exits normally. The native comparison includes an actual live read-budget object and admission-state changes, supplementing the synthetic width fixtures.

All 133 library tests pass in 10.04 seconds; the existing deliberately stalled welcome test accounts for ten seconds. Clippy lib/tests with warnings denied passes. JS interoperability passes all 65 checks in 1.126 seconds of test execution and returns all six checked resource gauges to zero. Builds/install/cleanup add wall time. Independent source parsing confirms all metric definitions/value expressions and group sizes [4,30,9] are preserved; it does not add a coherent-snapshot guarantee beyond the existing independent relaxed reads.

Both helper and native builds use clean copied committed sources plus only src/metrics.rs, with fresh:false and complete input/binary hashes. The helper example is diagnostic, not shipped into the runtime API. The frozen selected native executable retains its comparison path artifacts/candidates/metrics-static-20261019-native-candidate/war-relay. Existing manual services, convenience executables and three unrelated dirty runtime experiments are preserved.

Run `python docs/benchmarks/2026-10-19-static-metrics-prefixes/verify.py` to recompute medians, metadata/load-group preservation, profile totals, exact output/HTTP matches and source/build boundaries. Sources, helpers, commands, receipts and compressed profiles are retained. Repetition requires fresh output paths and sequential builds/runs. No multi-hour or full-population test was needed for this renderer change.
