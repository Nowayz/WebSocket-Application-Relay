# Direct scalar requests in the legacy bridge

The bridge now encodes listener flags and ID assignment directly into the final CBOR request. Both use one private `uint_request` helper. It removes temporary `protocol::uint` storage while preserving the complete canonical `[0, [op, uint]]` wire packet. Authentication, variable-read and variable-write encoder implementations are unchanged. No public API, unsafe code, runtime state, dependency or new protocol policy is introduced.

## Allocation evidence

Fresh release examples call the actual public `to_cbor` converter 1,000 times between external tcmalloc heap captures. Inputs and expected outputs are prepared and checked before measurement; a warm conversion establishes the input owner. Outputs are consumed through `black_box` and dropped. Candidate/control examples use identical probe source. No server or ordinary network timing is involved.

| 1,000 conversions | Control allocations / requested bytes | Direct scalar allocations / requested bytes |
| --- | --- | --- |
| Listener flags 7 | 3,000 / 38,000 | 1,000 / 5,000 |
| Assign ID 2^63 + 17 | 2,000 / 22,000 | 1,000 / 13,000 |
| Authenticate, 32 bytes | 1,000 / 38,000 | 1,000 / 38,000 |
| Read variable, 32-byte key | 1,000 / 38,000 | 1,000 / 38,000 |
| Write variable, 32 KiB value | 1,000 / 32,780,000 | 1,000 / 32,780,000 |

All probes have zero live object/byte deltas. In the listener control, symbolized stacks attribute 1,000 nine-byte buffer allocations and 1,000 24-byte ownership-metadata allocations to `protocol::uint`, plus the 1,000 five-byte final replies. The direct encoder removes both temporary allocations. The full-width integer control needs no spare-capacity metadata but still has a nine-byte temporary. Raw heap rows sum to their headers and capture hashes are retained. These requested-byte counts are not RSS savings.

## Short pinned conversion comparison

The accepted comparison pins each process to CPU 4, warms 10,000 conversions, then records seven short batches. Small commands use 100,000 conversions/batch; 32 KiB writes use 10,000. Four fresh processes per case run control/direct/direct/control. The table shows ranges of process medians; all samples are in `timing.json`. Builds, heap profiling and timing run separately.

| Case | Control ns/conversion | Direct scalar ns/conversion |
| --- | --- | --- |
| Listener flags 7 | 47.4–47.5 | 31.3–31.7 |
| Assign ID 2^63 + 17 | 41.3–41.9 | 32.6–32.8 |
| Authenticate, 32 bytes | 32.4–33.5 | 33.1–33.1 |
| Read variable, 32-byte key | 32.8–33.5 | 33.3–33.5 |
| Write variable, 32 KiB | 432.7–436.7 | 440.9–443.8 |

Listener conversion is about one third faster, and assignment about one fifth faster, in this probe. The unchanged large-write path has roughly 1–3% higher observed medians; no universal zero-regression or whole-bridge speedup is asserted. The selected gain is the scalar allocation reduction with a clear scalar conversion improvement. These are not network latency, native relay throughput, or 100k-user results.

A broader prototype shared the request-prefix constructor with byte/variable encoders. The initial unpinned screen was noisy. Pinned multi-batch runs then showed a repeated cost on byte commands. Forcing that shared helper inline improved scalar conversion but did not remove the byte-command cost, so both broader variants were rejected. The retained implementation restores byte and variable bodies exactly and adds only a scalar helper. Prototype timing records and the original shared-prefix patch against base `556bcdf` are archived. The inlined variant differs by `#[inline(always)]` on `request_encoder`; neither variant is selected.

## Wire and socket verification

The new scalar wire test covers flags 0, 7, 23, 24 and 255 and ID boundaries from zero through uint64 maximum, comparing complete packets against the prior public helper construction. Reserved-ID rejection remains the native relay's policy; translation does not silently reinterpret those values. Existing byte and variable boundary tests remain.

All **111 Rust tests pass in 0.56 seconds**, and Clippy passes with warnings denied. The supported checker passes all **61 JavaScript protocol/legacy/idle cases in 1.116 seconds**, including listener setup and privileged full-width ID takeover through the original client. All six resource gauges retire to zero. Debug socket fixtures are separate from the frozen release conversion examples. No existing manual service or convenience binary is promoted or restarted; pre-existing source overlays remain unchanged.

The frozen accepted examples are `artifacts/candidates/bridge-scalar-request-20261019-{control-v2,direct}/command_conversion`. Their source manifests differ only in `src/legacy_bridge.rs`; the probe source is identical. Earlier examples and local raw captures retain their artifact paths in receipts. Run `python verify.py` to audit the stored measurements.
