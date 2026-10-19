# Reject impossible channel-list schemas before allocation — 2026-10-19

A complete CBOR value can pass the generic item validator while containing the wrong channel-list element shapes. Previously, channel_reply allocated its destination from the declared array length before discovering that the first element was not a [name, population] record. A 30,000-element list of 255-byte byte strings is valid bounded CBOR but cannot be a valid channel reply; the converter still requested a 7,800,012-byte output buffer before rejection.

The selected check derives the maximum possible channel-record count from the existing protocol::MAX_ITEMS budget. The reply value consumes three items for its array, opcode and list; each channel record needs three more for its array, name and population. Thus count <= (MAX_ITEMS - 3) / 3 is necessary for any accepted record list. With the current 65,536-item budget, this is 21,844 records. Larger correctly shaped lists already fail generic validation. This is an early semantic check under the existing validator contract, not a new configurable quota or reduction of the accepted valid schema.

The check runs before destination allocation. Other representability/type checks and the original legacy layout remain unchanged. The existing wire-layout test is extended to the largest valid record count, with names spanning 0–255 bytes and populations across u32; the complete expected output still matches. No lock, queue, copy, new dependency or forwarding change is introduced.

The release diagnostic builds the same malformed envelope once, verifies it passes generic protocol::parse, and performs 0 or 64 rejected conversions under gperftools/tcmalloc. Subtracting the zero-conversion process removes packet-construction/startup allocations. All four raw profiles have header totals verified against every stack row. Small argument-size differences add 1/64 byte per conversion; requested-byte figures below are therefore approximate.

| Per rejected conversion | Previous | Selected |
| --- | ---: | ---: |
| Allocation events | 3 | 1 |
| Cumulative requested allocation bytes | 7,800,146 | 72 |

The large output allocation disappears; the selected error still allocates about 72 bytes. Both variants reject the exact same 7,710,007-byte input. End-of-process live heap is identical across each zero/64 pair, so this is allocation avoidance rather than a leak fix. Diagnostic allocator timings are excluded. This does not eliminate the already received upstream message or codec storage, and no RSS, CPU, throughput or population latency improvement is claimed.

All 133 library tests pass in 10.03 seconds, including the expanded 21,844-entry layout case and earlier ownership/FIFO tests. The prior intentional welcome-timeout test accounts for ten seconds. Clippy lib/tests with warnings denied passes. JS interoperability passes all 65 checks in 1.122 seconds of test execution, with all six checked resource gauges zero. Build/install/cleanup add wall time.

A separate frozen release bridge receives the malformed list from a fake upstream and preserves Close 1003 / Value Requires CBOR Client. A valid empty list still yields its exact 12 zero-byte legacy reply. Both cases have descriptor baseline/final 10/10, expected normal peer retirement and process exit zero. wire.json and wire.py retain the checked packet bodies and fixture. No existing services or convenience executables are replaced.

Both helper releases and the usable bridge release have fresh:false build receipts and input/binary hashes. Sources are copied from the committed tree with only the selected bridge change and diagnostic helper; three unrelated dirty runtime experiments are excluded. The bridge executable is artifacts/candidates/bridge-channel-count-20261019-selected/war-legacy-bridge.

Run `python docs/benchmarks/2026-10-19-bridge-channel-count/verify.py` for offline evidence checks. The helper, profiler runner, raw profiles, original/selected sources and wire fixture are retained. From the repository root, build channel_invalid_cost.rs as an example in separate clean source snapshots, then use the recorded 0/64 commands with the recorded tcmalloc library and fresh prefixes. No multi-hour or full-population test was needed for this malformed-reply allocation question.
