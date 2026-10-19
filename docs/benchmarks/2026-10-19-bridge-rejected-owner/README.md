# Release rejected fragmented upstream owners before Close — 2026-10-19

Selected: explicitly drop an unrepresentable binary upstream message or unexpected upstream text message before awaiting the legacy Close acknowledgement. Previously these owners remained in the bridge future during the existing bounded cleanup. The change adds two drop calls on error paths, with a named text binding; successful conversion/forwarding is untouched. No in-place rewriting, new copy, allocation, queue, timer, dependency or message limit is introduced.

This intentionally revisits a narrower case than the earlier rejected receive-ownership screen. That study measured successful single-frame receives and found shared codec storage plus no memory reduction from dropping input before successful sends. Here the input is terminally rejected and may be assembled from fragments. Dropping that separately assembled owner can release it while Close remains pending. The positive fragmented results do not reverse the negative single-frame conclusion.

A real-socket probe starts the frozen release bridge and fake upstream, joins one raw legacy client, and verifies welcome ID 42. The upstream sends either a valid CBOR structured map containing 4 MiB of bytes (unsupported by the legacy recipient) or an unexpected WebSocket text message containing 4 MiB of ASCII. Each is sent once as two fragments and once as a single frame, for both original and selected binaries. The client verifies the exact Close and withholds acknowledgement while the probe captures live requested heap. It then acknowledges, verifies upstream EOF, retires peers and checks descriptors return from 10 to 10 with process exit zero. There are eight short process cases total.

Only the bridge uses the replacement tcmalloc allocator and SIGUSR2 gate captures. The profiler is diagnostic: timings/RSS are excluded. Every raw profile header is independently checked against its full stack rows. Joined-gate live bytes are 106,605 in every case. Retired backgrounds vary by 128 bytes in some runs; the released-owner column compares (closing minus retired) between variants to remove that small background offset.

| Rejected upstream input | Original closing live bytes | Selected closing live bytes | Released owner bytes, retired-adjusted |
| --- | ---: | ---: | ---: |
| Fragmented structured binary | 6,398,168 | 2,203,854 | 4,194,314 |
| Single-frame structured binary | 4,301,011 | 4,301,011 | 0 |
| Fragmented text | 6,398,079 | 2,203,903 | 4,194,304 |
| Single-frame text | 4,301,055 | 4,300,927 | 0 |

The binary message includes ten CBOR envelope/map/byte-string header bytes in addition to the 4 MiB data. Fragmented text releases exactly 4 MiB. Shared single-frame receive storage remains owned by the codec even after dropping the message view, so there is no corresponding reduction. Codec/runtime memory remains at Close; this is neither a leak fix nor a claim to free all transport memory. Normal allocator RSS need not fall when an owner is released.

Every binary case preserves Close 1003 / Value Requires CBOR Client. Every text case preserves Close 1002 / Invalid Upstream Message. All eight release cases pass exact Close, EOF, descriptor-retirement and process-exit checks. Existing valid-data FIFO/ownership tests remain unchanged. All 133 library tests pass in 10.03 seconds; the previous deliberate stalled-welcome case accounts for ten seconds. Clippy lib/tests with warnings denied passes. JS interoperability passes 65 checks in 1.121 seconds of test execution with all six checked resource gauges zero. Builds/install/cleanup add wall time. No throughput or 100,000-user performance claim is made.

Both release identities and full source hashes are retained. All runtime src/ inputs differ only in legacy_bridge.rs; the original snapshot also contained an unrelated diagnostic example, which is not part of this bridge's runtime. Selected/original source comparison confirms only the rejected-owner drop changes and unchanged tests. Snapshots exclude the three unrelated dirty runtime experiments. Manual services and convenience executables were preserved. The selected frozen bridge is artifacts/candidates/bridge-rejected-owner-20261019-selected/war-legacy-bridge.

Run `python docs/benchmarks/2026-10-19-bridge-rejected-owner/verify.py` for offline evidence checks. From the repository root, replay probe.py with `--bridge FROZEN_BINARY --kind binary|text [--fragmented] --output FRESH.json`; it uses ephemeral loopback ports and requires the recorded tcmalloc library. Use fresh outputs and run sequentially. Raw profiles, all receipts, sources, patch and the exact probe are retained.
