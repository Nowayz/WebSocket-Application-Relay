# Reject a direct byte-string parser path

The current relay remains unchanged. A five-line specialization in `protocol::parse` speeds isolated definite-byte-string parsing by about 30%, but does not establish a whole-relay improvement. Reject it rather than retain an additional validation path with an unproven application benefit.

The previous 100,000-user profile places 16.74% of sampled cycles in bounded receive/routing. That symbol includes parsing, locking, lookup, ownership and submission; it is not a parser-only percentage. The earlier broad leaf-validator experiment was already rejected. This screen tests a narrower alternative: after reading the envelope target, recognize a definite byte-string header, call the existing checked `Decoder::bytes`, and retain the ordinary end-of-input check. Every other type still uses the existing bounded validator. No unsafe code, custom length decoder, payload copy, cache, dependency, or queue policy is introduced.

## Short isolated parser screen

Fresh processes run control/direct/direct/control, pinned to CPU 7. Each process measures nine batches of one million parses per payload size in an optimized release executable. Input and results pass through `black_box`; the complete parsed sender/value is checked before timing. Values use canonical CBOR byte strings. Each parse includes envelope/target decoding, validation and trailing-byte detection; payload contents are not scanned because CBOR byte strings are opaque. Numbers are process medians in ns/parse, not relay latency.

| Payload bytes | Control 1 | Direct 1 | Direct 2 | Control 2 |
| --- | ---: | ---: | ---: | ---: |
| 0 | 14.70 | 10.12 | 9.86 | 14.88 |
| 16 | 14.78 | 10.10 | 9.88 | 14.81 |
| 1024 | 15.12 | 10.57 | 10.35 | 15.26 |
| 65536 | 15.13 | 10.62 | 10.40 | 15.22 |

The absolute gain is only roughly 4.5 ns per parse. This is not a reason by itself to complicate the production parser.

## Three-second real-relay comparison

Four new processes run in the same order. Every case uses 8,192 stable rooms, 64 private pairs, 1 KiB cyclic templates, 16-message windows, four server workers on cores 0–3 and eight client workers on 4–7,20–23. Native and client workers are individually pinned and verified at the ready/mixed/done gates. No client shares a physical core with the server. Churn and active inspection are disabled in all four cases; full room census and normal Close/resource checks still run before/after measurement. This is a saturated private traffic screen, not another 100,000-user fairness test.

| Trial | Variant | Million checked deliveries/s | Server ns/delivery | Client-observed p99 upper, ms |
| --- | --- | ---: | ---: | ---: |
| 1 | Control | 2.344 | 1144.69 | 1.049 |
| 2 | Direct | 2.338 | 1150.34 | 1.049 |
| 3 | Direct | 2.333 | 1144.12 | 1.049 |
| 4 | Control | 2.320 | 1153.34 | 1.049 |

Ranges overlap and the p99 bucket is identical. The bucket is an upper bound, not an exact quantile. Maximum latencies vary, and two repetitions cannot establish a rare-tail improvement. No consistent application speedup is demonstrated. These trials verify **28,011,216 timed private deliveries** in total, plus separately counted setup and postcheck exchanges.

Both binaries and benchmark executables were built sequentially from clean revision `bac263fab56b265b961cb6d8f4d23a326480eac8` in the same project/target paths and release profile. The only library source difference is the archived `protocol::parse` specialization. The three pre-existing user overlays were excluded. No builds/profiles overlap timed runs. No candidate binary replaces a running service or the convenience release executable.

An independent 40-case smoke corpus also passes: 103 whole/fragmented/pipelined exchanges, 51 checked healthy witness messages, explicit policy/decoder Close outcomes, and resource/counter gates. This is a finite smoke check, not exhaustive protocol equivalence or a full fuzzing campaign; the candidate is rejected before broader validation. The isolated byte-string measurements do not characterize text/scalar/container performance.

## Reproduce and audit

`python3 docs/benchmarks/2026-10-18-direct-bytes-screen/verify.py` checks archived hashes, matched source difference, exact relay counts/resource gauges, parser batch shapes, and the corpus result.

To reproduce builds, extract `git archive bac263f Cargo.toml Cargo.lock src examples vendor` into a fresh directory, copy `source/parse_screen.rs` into its examples directory, then build with `cargo build --release --example parse_screen --bin war-relay`. Freeze the control executables, replace only `src/protocol.rs` with `source/protocol-direct.rs`, and rebuild in the same paths. The probe needs no dependencies beyond this project. Pin to an isolated physical core and retain all nine raw batches.

Full server/client CLI commands, hashes, pinned affinity observations, setup/mixed/done snapshots and exact expected counters are in the four relay JSON receipts. Repeat them with fresh ports/output names through `scripts/inspection.py`; the archived runner source pins the historical behavior. CPU topology is host-specific, so inspect it before adapting the command. `summary.json` contains derived results, and source snapshots make the rejected experiment reviewable without modifying the retained runtime.
