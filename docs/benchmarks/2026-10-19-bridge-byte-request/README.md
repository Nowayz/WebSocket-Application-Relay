# One-buffer byte-valued bridge commands

Authentication and variable-read conversion previously built an encoded CBOR byte string, then copied it into the complete relay request. They now share a private `byte_request` helper that reserves the exact complete request size and encodes `[0, [op, bytes]]` directly into that one buffer. The helper supports the full uint8 opcode width, although current callers are operations 0 and 5. No new public API, state, dependency, unsafe code or lock is introduced. The final packet still copies the legacy input once; this is not end-to-end zero-copy.

## Focused allocation comparison

Two separate server-free release probes execute the actual public `to_cbor` conversion 1,000 times: one authenticates with a 32-byte input and one gets a variable using a 32-byte key. Each validates complete output against the existing public protocol helpers, warms the input's shared ownership, and consumes/drops every output through `black_box`. External tcmalloc captures are taken at stdin gates. Every raw row sums to its header, with recorded hashes and symbolized stacks.

| Each 1,000-conversion probe | Control | Direct helper |
| --- | --- | --- |
| Allocation objects | 2,000 | 1,000 |
| Requested allocation bytes | 72,000 | 38,000 |
| Live object / byte delta | 0 / 0 | 0 / 0 |

Both commands remove exactly **one allocation and 34 requested bytes per 32-byte input conversion**. These measurements cover the conversion function, not complete bridge memory or network timing.

Four release processes per case run control/candidate/candidate/control with inputs prepared outside timing. Small inputs use 100,000 conversions; the 4 KiB case uses 10,000. Builds, heap profiling and ordinary timings run separately.

| Case | Control ns/conversion | Direct helper ns/conversion |
| --- | --- | --- |
| Authentication, 32 bytes | 49.2–54.0 | 41.8–44.4 |
| Authentication, 4,096 bytes | 129.6–135.2 | 86.1–91.8 |
| Variable read, 32-byte key | 54.6–56.1 | 34.8–40.1 |
| Variable read, 255-byte key | 65.7–67.5 | 44.6–51.1 |

These isolated conversion results justify the shared direct encoder; they establish no whole-relay throughput, response-latency SLA or mass-user improvement.

## Wire behavior and integration

The new wire test covers byte-string length-width transitions for both commands, empty authentication, and the existing ignored-empty-key behavior for variable reads. Authentication lengths extend through the supported 4,096-byte secret boundary; variable key cases remain at most 255 bytes. Every complete packet matches the prior protocol-helper construction.

All **110 Rust library tests pass in 0.50 seconds**, and Clippy passes with warnings denied. The supported checker runs all **61 JavaScript protocol/legacy/idle cases in 1.126 seconds**, including original-client authentication and variable round trips. All six checked native resource gauges return to zero. The checker builds and runs debug fixture binaries; the separately frozen release conversion examples provide the optimized allocation/timing evidence. No new release service is promoted or restarted.

The examples are frozen in `artifacts/candidates/bridge-byte-request-{control,candidate}/byte_command_conversion`. Build manifests, raw diagnostic receipts and exact timings are retained here. The candidate source matches the selected conversion implementation and boundary test. Existing manual services and pre-existing dirty Rust/C++ files remain untouched. Run `python verify.py` for the evidence audit.
