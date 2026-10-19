# Encode legacy variable writes directly into final storage

Legacy `SetVariable` conversion previously encoded `[key, value]` into an intermediate vector, then copied that encoding into a second vector for `[0, [4, [key, value]]]`. The bridge now reserves the exact complete request size and writes the full request directly into it. Key/value byte-string semantics, wire bytes, validation, FIFO and downstream bounds are unchanged. No pooling, unsafe code, lock or dependency is added. The final owned CBOR packet still copies the original legacy key/value bytes once; this is not end-to-end zero-copy.

## Direct allocation measurement

A freshly compiled release probe calls the actual public `legacy_bridge::to_cbor` conversion 1,000 times using an already owned legacy input with a three-byte key and 32,768-byte value. It checks exact output against the existing protocol helpers before measurement, warms the input's shared ownership, then waits at stdin gates around external tcmalloc captures. Output is consumed through `black_box` and dropped each iteration. Only the probe process receives the replacement allocator.

| 1,000 conversions | Control | Direct encoder |
| --- | --- | --- |
| Allocation objects | 2,000 | 1,000 |
| Requested allocation bytes | 65,571,000 | 32,780,000 |
| Live object/byte delta | 0 / 0 | 0 / 0 |

This removes exactly **one allocation and 32,791 requested bytes per conversion** for this key/value size, corresponding to the old `20 + key.len() + value.len()` intermediate reservation. All raw heap rows sum to their headers, hashes are recorded, and stack addresses are symbolized against the frozen executables. These are isolated conversion allocations, not whole-bridge RSS or network timings.

## Short conversion timings

Four separate release processes per size ran control/candidate/candidate/control. The probe prepares and validates inputs outside timing, warms 1,000 conversions, then consumes each complete public conversion. Small values use 100,000 measured conversions and values above 1 KiB use 10,000. No build or profiler overlaps ordinary timing.

| Value bytes | Control ns/conversion | Direct encoder ns/conversion |
| --- | --- | --- |
| 0 | 45.2–49.5 | 49.1–50.6 |
| 24 | 46.2–57.6 | 46.7–56.7 |
| 1,024 | 70.9–74.9 | 61.6–63.6 |
| 32,768 | 661.2–667.0 | 432.9–439.4 |
| 65,536 | 1,217.2–1,235.0 | 748.5–809.6 |

The 32 KiB conversion is about one third faster in this probe. Empty values do not improve and show a small cost in these observations; no universal latency improvement is claimed. The retained benefit is one fewer allocation and removal of the second key/value copy, with a substantial conversion win for larger writes. This is not relay throughput, mass-user fairness or a network latency SLA.

## Behavior and integration

A focused wire test covers key lengths 1, 23, 24 and 255 and value lengths 0, 23, 24, 255, 256, 65,535 and 65,536. It compares the complete packet against the previous public protocol-helper construction, covering CBOR length-width transitions. All **108 library tests pass in 0.50 seconds**, and Clippy passes with warnings denied.

The existing original-JavaScript-client integration case now stores and reads back changing binary values of lengths 0, 23, 24, 255, 256 and 32,768 through the freshly frozen bridge. All **61 JavaScript socket cases pass in 1.116 seconds**, with both fixture processes exiting zero and all six checked native resource gauges returning to zero. The unchanged native fixture is the selected inline-u64 build. Existing manual services and unrelated dirty files are preserved.

The selected bridge is `artifacts/candidates/bridge-variable-write-selected/war-legacy-bridge`. The allocation/timing examples are separately frozen at `artifacts/candidates/bridge-variable-write-{control,candidate}/variable_conversion`; their production conversion source matches the tested branch, before the new unit test was added. Raw capture paths and hashes remain in the receipts. Probe sources and exact timing results are retained here. Run `python verify.py` for the offline evidence audit.

Report-label correction: the analyzer originally inherited a generic 64-operation scope label from an older probe. The actual operation count was always 1,000 in each recorded probe and in all calculations above. The archived analysis scope labels are corrected; raw captures, counts, byte totals and conclusions are unchanged.
