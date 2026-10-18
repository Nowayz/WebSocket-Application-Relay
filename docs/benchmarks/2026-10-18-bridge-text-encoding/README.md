# Single-buffer legacy text encoding

Adopt code commit `4c9113a`: ordinary nonzero-target legacy text packets now encode their CBOR envelope and string directly into one exactly sized final buffer. This follows the [binary encoding change](../2026-10-18-bridge-fused-encoding/README.md). UTF-8 byte length determines capacity and the encoded string length. The previous temporary encoded value and second payload copy are removed. Binary forwarding, command handling, reverse conversion, frame limits, NODELAY and the 4 KiB initial upstream read buffer are unchanged. No new API, dependency, allocator, buffer pool or unsafe code is introduced.

## Short component measurements

Both variants and the probe are sequential release builds in the same clean project/Cargo target paths, starting from `bc58d9c`. Fresh processes run control/direct/direct/control on CPU 7, after all builds finish. Inputs are prepared outside timing and clone the same shared `Utf8Bytes`; results are checked against literal CBOR headers and full original payloads before timing. Each case warms up 1,000 calls and records nine batches including conversion/result destruction with opaque inputs/outputs. The target is the same full-width `0x0102030405060708` in all cases.

| Case | Payload bytes | Control process medians, ns/call | Direct process medians, ns/call |
| --- | ---: | ---: | ---: |
| ASCII | 1 | 58.05 / 59.00 | 45.11 / 43.83 |
| ASCII | 16 | 59.37 / 59.31 | 44.16 / 44.21 |
| Repeated lambda | 1024 | 86.70 / 85.04 | 52.26 / 52.33 |
| Repeated crab emoji | 65536 | 1415.29 / 1415.98 | 748.86 / 740.36 |

The two-repeat means show about **39% less conversion time at 1 KiB** and **47% at 64 KiB**. These are component results, not whole-relay throughput or latency percentages. Each batch has 100,000 calls except the 64 KiB case, which has 2,048. All raw batches are retained.

## Allocation-count diagnostic

Separate fresh probe runs preload installed gperftools `libtcmalloc` and enable complete heap profiling. Those allocator-replacement timings are excluded. Each variant executes 2,722,436 conversions including validation/warmup/batches:

| Variant | Cumulative allocations | Cumulatively allocated bytes |
| --- | ---: | ---: |
| Control | 8,167,357 | 4,544,877,733 |
| Direct | 2,722,485 | 2,243,534,216 |
| Difference | **5,444,872** | **2,301,343,517** |

That is exactly **two removed allocations per conversion** for this measured 64-bit-target workload. The old text helper reserves nine header bytes even when fewer are encoded. In pinned `bytes` 1.12.1, that spare capacity needs shared-storage metadata; fusing removes both the temporary value buffer and its metadata allocation. The byte delta equals the temporary `payload + 9` allocation plus 24 bytes of metadata per call on this recorded 64-bit build. Both profiles include the same 49 nonconversion allocations. These are cumulative allocation traffic counts, not RSS or peak-memory savings. The selected service retains its prior allocator environment.

## Real forwarding screen

Four fresh control/direct/direct/control cases each connect 256 distinct legacy rooms and verify 512 self echoes per client: **131,072 per run**. Each 1 KiB UTF-8 payload has a changing 16-byte ASCII sequence plus repeated lambda characters; the entire returned sender prefix/body and FIFO are checked. The same frozen native relay and four bridge runtime workers are used. Processes request disjoint masks: native 0–3, bridge 4–7, generator 8–15. CPU counters bracket quiescent joined/warmed gates, with 10 ms tick resolution.

Observed bridge CPU is **1.11 / 1.08 / 1.08 / 1.12 CPU-s**. Both direct repeats are slightly lower in this short screen. This is a small finite CPU observation, not a statistically established general speedup, saturation-throughput result, 100,000-user fairness test or latency guarantee. Python generation and socket/client scheduling affect wall time. Allocation and component results provide the primary selection evidence.

Additional direct-build checks verify 512 full 64 KiB UTF-8 echoes and 64 full 1 MiB echoes; the final selected executable repeats the 1 MiB case. Exact native application/welcome/close counts and final resource gauges pass. Routing copied bytes and unrelated error/pressure counters remain zero. The native relay's zero-copy forwarding is unchanged. Compatibility conversion still copies the string into a contiguous output message; this change removes redundant work rather than making every bridge operation zero-copy.

## Semantics and acceptance

- Literal tests cover 21 target/UTF-8 byte-length combinations through 65,536 bytes, including small IDs and broadcast. They check full wire bytes, unchanged input and independently owned output with no spare capacity. A separate literal checks NUL, lambda, emoji and combining characters without normalization. Empty legacy payloads and malformed base64 remain rejected; valid zero-target text remains ignored.
- Clean library: **80 passed**. Integrated workspace, including the preserved three existing index tests: **83 passed**. Strict clean library/bridge Clippy and changed-source Rust 2024 formatting pass.
- Real JavaScript/native/legacy suite: **7 passed, none skipped**, on both direct and final selected builds. This includes binary/text interoperability, typed values/variables, privileged commands, fixed-ID takeover, unsupported structured values, normal close and idle behavior.
- The final selected binary passes the controlled delayed-ACK regression, preserving NODELAY. The preceding binary/frame-limit validation remains linked above; this change touches only outbound legacy-text conversion.
- Guarded local adoption checked the old bridge identity, no connected bridge clients and zero native live-state gauges. It replaced only the bridge, keeping native PID 966632 and static UI PID 219427. Live 1 KiB private and room-broadcast checks pass in **both binary and UTF-8 text modes**, followed by departure, two exact normal Close/EOF exchanges and zero final resource gauges. The convenience release binary was aligned atomically. No hosted deployment occurred.

Selected executable SHA-256: `bb2392f53bd8fe1bdff47f0a8e790b366269400e259839565bdeb4573f9f8bf1`. The clean selected `src` tree was verified against `4c9113a`; all pre-existing user overlays were excluded and remain byte-identical. `build.json` pins source/binary/compiler identity. The selected source adds an explanatory comment and cfg(test) UTF-8 tests to the measured runtime body. Final selected receipts separately pin its executable.

## Reproduce and verify

`python3 docs/benchmarks/2026-10-18-bridge-text-encoding/verify.py` checks hashes, runtime source correspondence, batch shapes, exact allocation/byte deltas, wire/counter/resource gates, tests, selected compatibility and live adoption receipts. No network or running server is needed.

To rebuild, extract `git archive bc58d9c Cargo.toml Cargo.lock src examples vendor`, add `source/bridge_text_screen.rs` under examples, and build `cargo build --release --example bridge_text_screen --bin war-legacy-bridge`. Freeze the control executables, replace only `src/legacy_bridge.rs` with `source/measured-direct.rs`, and rebuild in the same project/target paths. Use clean revision `4c9113a` for the selected artifact. Build/profile work never overlaps ordinary timed phases.

Wire and selected-JS helpers expect copies under the repository's `artifacts/` directory and fresh output names. The Unicode wire helper uses even payload byte lengths at least 16 for its sequence-plus-lambda shape; it is a specific recorded workload, not a general text generator. Full commands, counters and process snapshots are in receipts. Heap environment is recorded in `provenance.json`; keep diagnostic preloads out of ordinary comparisons and service adoption. Only the separately archived guarded promotion helper mutates local services.
