# Single-buffer legacy binary encoding

Adopt code commit `1475c49`: ordinary nonzero-target legacy binary packets now encode the CBOR array, target and byte string directly into one exactly sized final buffer. Previously the bridge allocated/filled an encoded byte-string value, then allocated the envelope and copied that value again. The conversion now copies the application payload once into final storage. Text, command, reverse conversion, frame limits, native routing, upstream TCP_NODELAY and the 4 KiB initial upstream read buffer are unchanged. No in-place mutation scheme, new public API, allocator, dependency or unsafe code is introduced.

## Isolated conversion measurements

Both release variants and the conversion probe are built sequentially in the same clean project/Cargo target paths. Base revision is `3737ced`. Fresh processes run control/direct/direct/control, pinned to CPU 7. The probe constructs input outside timing, checks the complete result against independently written literal CBOR headers/data, performs 1,000 warmup conversions, then records nine batches. Each batch includes conversion and result destruction with opaque input/output. The target is `0x0102030405060708`; shared input cloning is identical in both variants.

| Payload bytes | Control process medians, ns/conversion | Direct process medians, ns/conversion |
| --- | ---: | ---: |
| 1 | 39.74 / 41.32 | 28.45 / 28.35 |
| 16 | 39.17 / 39.39 | 29.82 / 29.86 |
| 1024 | 57.68 / 57.39 | 39.16 / 39.04 |
| 65536 | 1333.19 / 1328.29 | 775.37 / 720.39 |

For these two-repeat means, 1 KiB conversion time falls about 32%, and 64 KiB about 44%. These are component measurements, not relay throughput or end-to-end latency percentages. Batches have 100,000 operations except 64 KiB batches, which have 2,048. All raw batches remain available.

## Allocation-count diagnostic

Separate fresh runs preload the installed gperftools `libtcmalloc` only into the probe and enable complete heap profiling. Profiling replaces its allocator; those timings are excluded from the ordinary comparison. Each variant makes 2,722,436 conversions including validation, warmup and all batches. The final cumulative heap-profile totals are:

| Variant | Allocation count | Cumulatively allocated bytes |
| --- | ---: | ---: |
| Control | 5,444,917 | 4,459,572,902 |
| Direct | 2,722,481 | 2,243,467,603 |
| Difference | **2,722,436** | **2,216,105,299** |

That is exactly one removed allocation per conversion in the measured 64-bit-target workload. The byte difference equals all intermediate encoded byte-string sizes across those calls. These are cumulative allocation traffic totals, not peak memory or RSS savings. Both profiles include the same 45 nonconversion allocations. The selected service retains its prior allocator environment.

## Real wire checks and practical limit

Four fresh control/direct/direct/control processes connect 256 distinct legacy rooms and perform 512 checked 1 KiB self echoes per client, **131,072 per run**. The same selected native relay, four bridge runtime workers and full packet/sender/sequence/FIFO checks are used. Native, bridge and generator processes request disjoint masks 0–3, 4–7 and 8–15 respectively. CPU clocks bracket quiescent joined/warmed gates; Python generation and socket scheduling affect wall time. This is not a saturation or 100,000-user fairness test.

Bridge CPU totals are **0.96 / 0.95 / 0.96 / 0.96 CPU-s**, sampled with 10 ms tick resolution. Wall times are **1.391 / 1.315 / 1.326 / 1.294 s**. No repeatable whole-bridge CPU or latency gain is established. The change is retained for the measured public conversion improvement and elimination of an avoidable allocation/copy, without claiming a full-relay speedup.

Additional direct-build checks verify 512 full 64 KiB echoes and 64 full 1 MiB echoes. The clean selected executable repeats the 64-echo 1 MiB check. All native packet totals, welcomes, normal closes and resource retirement pass; routing copied bytes and unrelated error/pressure counters remain zero. Native forwarding ownership/FIFO is unchanged. The bridge still requires a contiguous converted message for its WebSocket sink; this is not a claim that all compatibility forwarding is zero-copy.

## Correctness and adoption

- A literal wire test covers ten target integer encodings and seven byte-string length boundaries through 65,536 bytes, including broadcast. It checks all 70 complete encodings, original input preservation, independent output ownership and absence of spare output capacity. The original empty-payload rejection remains checked.
- Clean isolated library: **79 passed**. Integrated workspace, including the three preserved existing index tests: **82 passed**. Strict clean library/bridge Clippy and changed-source Rust 2024 formatting pass.
- Real JavaScript/native/legacy suite: **7 passed, none skipped**, including idle policy, text/bytes/uint64/variables, privileged commands, ID takeover, normal closes and unsupported structured values. The final clean selected executable repeats all seven successfully.
- Controlled delayed-ACK checks pass for both direct and selected executables, preserving the previous NODELAY behavior. Independent upstream large-frame checks pass through the existing exact 16 MiB frame boundary for both builds. These are compatibility checks, not large-frame speedup claims.
- Guarded local promotion verified the old bridge identity, no connected bridge clients and zero native live-resource gauges, then replaced only the bridge. Native PID 966632 and static UI PID 219427 stayed running. Live private/broadcast 1 KiB delivery, departure notice, two exact normal Close/EOF exchanges and zero final resource gauges passed. The convenience release bridge binary was aligned atomically. No hosted deployment occurred.

Selected executable SHA-256: `d9de976c3fd04ed565b92604f80f95cfe961347c2a0ea2bb133ceb5670569719`. Its `src` tree was verified against Git revision `1475c49`; all user overlays were excluded from that build and remain byte-identical in the workspace. `build.json` records compiler and source/binary hashes. The selected source adds only an explanatory comment and cfg(test) boundary test to the measured direct source's runtime body; final selected receipts pin its distinct executable separately.

The first conversion probe tried an empty payload, which the existing legacy protocol rejects as a header-only packet. The control failed before any timing output. That invalid probe source and explanation are retained; valid measurements use one byte as the minimum, and runtime rejection behavior was not changed.

## Reproduce and audit

`python3 docs/benchmarks/2026-10-18-bridge-fused-encoding/verify.py` verifies file hashes, batch shapes, exact allocation/byte deltas, runtime source correspondence, real-wire counter/resource gates, compatibility receipts and local adoption. No network or server process is needed for that audit.

For builds, extract `git archive 3737ced Cargo.toml Cargo.lock src examples vendor`, copy `source/bridge_encode_screen.rs` into examples, and build `cargo build --release --example bridge_encode_screen --bin war-legacy-bridge`. Freeze control executables, replace only `src/legacy_bridge.rs` with the measured direct snapshot, and rebuild in the same project/target paths. To build the selected artifact, use clean revision `1475c49` instead. Cargo release profile and dependencies are unchanged. No builds/profiles overlap ordinary timing.

The source directory also contains the wire, large-frame, delayed-ACK, selected JS and guarded-promotion helpers. Their path resolution expects copies under the repository's `artifacts/` directory; use fresh output paths and explicit executable arguments. CPU masks describe this host. Heap commands/environment are recorded in `provenance.json`; keep preload profiling separate from ordinary timings and runtime adoption. Only the guarded promotion helper is mutating; ordinary probes use disposable ports/processes.
