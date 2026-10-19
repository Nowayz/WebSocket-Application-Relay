# Single-buffer legacy channel replies — 2026-10-19

Selected: convert channel-list replies into one bounded output buffer instead of growing separate name and population vectors and copying them into another vector. Preserve the exact original layout: eight zero bytes, little-endian u32 count, all one-byte-length-prefixed raw names, then all little-endian u32 populations. Native routing is unchanged.

The converter decodes each entry once. Names grow forward; population words are written after a safe upper bound on the name block. At completion, one overlapping move places those words immediately after the actual names, and the vector is truncated. This removes the separate staging vectors and repeated growth; it does **not** make legacy conversion zero-copy. Names copy into the owned output once, and the population block moves once. The input remains borrowed and unchanged. No new lock, queue, dependency or public API is added.

The name bound is the smaller of the remaining encoded-entry bytes and `count * 256`, because each legacy name has a one-byte length and at most 255 raw bytes. Valid encoded entries necessarily occupy at least their name-block size. `protocol::parse` already bounds the envelope to 65,536 CBOR items, bounding entry count and arithmetic; an oversized invalid name cannot force an allocation proportional to that name's encoded size. Existing rejection of names above 255 bytes, populations above u32, malformed entry arrays and unsupported types is preserved.

The vector retains some upper-bound capacity after truncation. Converting a vector with spare capacity to Bytes also allocates its shared-owner metadata. For 1,024 channels with 16-byte names, the selected buffer requests 25,332 bytes and its owner metadata adds 24 bytes, while the final wire body is 21,516 bytes. This is a deliberate bounded-capacity tradeoff to avoid another decode pass and repeated allocation; no general peak-RSS reduction is claimed.

## Measurements

A frozen release example prepares CBOR input and an independent expected legacy body before timing. Each timed conversion checks the complete output bytes, then drops the output. It exercises 0, 1, 64, 1,024 and 4,096 channels, with 16-byte raw names and varying populations. It measures the public conversion helper plus common byte verification, not end-to-end WebSocket latency. All builds, ordinary runs and allocation profiles were sequential; no profiler overlaps ordinary timing.

Each size is measured three times per binary in control/final/final/control/control/final order, then three times per binary with the order reversed. Both executables use the same CPU, recorded in receipts. The table gives median microseconds per conversion from the first sequence and percentage elapsed changes from both sequences; negative is faster.

| Channels | Control µs | Selected µs | First elapsed change | Reverse-order elapsed change |
| --- | --- | --- | --- | --- |
| 0 | 0.053 | 0.043 | -19.14% | -19.64% |
| 1 | 0.115 | 0.068 | -41.35% | -41.22% |
| 64 | 1.654 | 1.378 | -16.66% | -17.50% |
| 1,024 | 23.200 | 22.345 | -3.69% | -5.32% |
| 4,096 | 90.311 | 88.715 | -1.77% | -1.27% |

These finite helper measurements establish a local conversion benefit, not a relay throughput or tail-latency guarantee. The larger cases improve modestly; the most substantial elapsed benefit is for small lists.

Separate gperftools allocation profiles run 0 and 1,000 conversions with identical fixtures, subtracting the zero-conversion process totals. At 1,024 channels, the original has **27 allocation/reallocation events per conversion and approximately 95,299 cumulative allocated bytes**. The selected implementation has **two events and approximately 25,356 bytes**, a 92.6% event reduction and 73.4% byte reduction. Small argument/formatting differences contribute a few bytes across the whole process, so byte deltas are approximate. These are cumulative requested allocations, not live/peak memory or the system allocator's timing.

An earlier two-pass exact-size candidate achieved one allocation but slowed 1,024-/4,096-channel conversions by about 25–27%. It remains unselected. Its source, build manifest and receipts are retained, even though its exploratory local directory was named `selected`. Only `final-build.json` and `legacy_bridge.rs` describe the selected implementation. Intermediate single-pass measurements before adding the explicit name-width bound are not used for the final table.

## Verification and reproduction

All **120 library tests passed in 0.54 seconds**, strict bins/lib/tests Clippy passed, and **65 JavaScript checks passed in about 1.14 seconds**, with six checked resource gauges zero. JavaScript coverage includes the real legacy privileged channel-list layout/FIFO barrier through a native relay and bridge. The focused Rust test independently checks complete wire bodies for empty, single, 64 and 1,024-entry lists; names span lengths 0–255 and arbitrary raw bytes, with populations reaching u32::MAX. It also rejects 256-byte/1-MiB names and populations above u32. Those compatibility tests apply to the old implementation too; this is an optimization rather than a behavior fix.

`build-snapshot.py` manifests pin clean copied inputs and require fresh release artifacts. `provenance.json` records selected source, compiler and profiler identities. `run.py`, `repeat-run.py`, the example, raw compressed heap profiles, complete receipts and the patch are retained. Run `python docs/benchmarks/2026-10-19-bridge-channel-reply/verify.py` to audit hashes and recompute results. Rebuild the example against each source variant in a fresh snapshot, then use the recorded commands for independent measurement; choose fresh output/profile names.

The previously running services and three pre-existing worktree edits were preserved. No multi-hour or full-population test was needed to answer this conversion-specific question.
