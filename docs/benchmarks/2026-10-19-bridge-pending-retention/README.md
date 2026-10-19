# Release buffered application ownership before terminal Close — 2026-10-19

The pre-welcome bridge may retain one frontend application message to preserve pipelining/FIFO. When upstream setup fails or a terminal upstream message wins before that buffered message is forwarded, it is no longer deliverable. Previously its owner remained in the bridge future while error cleanup waited for Close acknowledgement, potentially for the existing ten-second closing budget.

The selected change explicitly drops pending.take() before eight terminal cleanup paths: pre-welcome upstream Close or mapped framing error; welcome phase failure; invalid welcome; and post-welcome unsupported CBOR, mapped upstream framing error, unexpected upstream text or upstream Close before dispatch of the retained frontend message. Valid welcome/forwarding keeps its existing ownership. EOF/error paths with no subsequent await already release the future directly. No new queue, timer, allocation, copy, configuration or dependency is added. Protocol Close codes/reasons and FIFO are unchanged.

A controlled real-socket requested-heap diagnostic uses the existing release bridge, one frontend client and a fake upstream. After the native subscription, the frontend sends a fragmented 4 MiB application message; the upstream withholds welcome. The probe captures a pending gate, then sends reserved welcome ID zero. It verifies Close 1002 / Invalid Upstream Welcome on both transports, acknowledges upstream immediately, and withholds frontend acknowledgement while capturing the closing gate. It then acknowledges the frontend and checks both peer retirement, owned process exit and descriptors returning from 10 to 10. There is a 50 ms diagnostic settling interval before the pending capture, not a production delay. All phases finish inside the existing setup/Close budgets.

Only the bridge allocator is replaced with gperftools/tcmalloc for SIGUSR2 quiescent heap dumps. Header totals are recomputed from every raw stack row. These are live requested allocations, not system-allocator RSS or timing measurements. The raw profiles, original hashes and profiler identity are retained.

| Gate | Original live requested bytes | Selected live requested bytes |
| --- | ---: | ---: |
| Joined, before application message | 106,555 | 106,555 |
| Application message pending | 6,398,011 | 6,398,139 |
| Closing, frontend acknowledgement withheld | 6,398,095 | 2,203,919 |
| Retired | 91,083 | 91,211 |

The original pending-to-closing change is +84 bytes; selected is -4,194,220 bytes. Their difference is exactly **4,194,304 bytes (4 MiB)**, the buffered message size. The 128-byte cross-process background difference appears at pending and retired gates and cancels in that comparison. The remaining roughly 2.2 MB at selected closing includes codec/runtime state; this is not a claim to release all transport memory before Close completes. Both variants preserve the same verified wire result and retire their descriptors normally. One earlier diagnostic attempt failed parsing a heap header before sending the application message; its failed receipt is retained and excluded. The parser was corrected and fresh output paths used for the accepted runs.

All 133 library tests pass in 10.05 seconds, including binary/text pending-message FIFO and single-slot retirement/recovery. The existing deliberate stalled-welcome test accounts for ten seconds. Clippy lib/tests with warnings denied passes. JS interoperability passes 65 checks in 1.121 seconds of test execution and returns all six checked resource gauges to zero. Builds/install/cleanup add wall time. No full-population or throughput claim is made for this terminal optional-bridge ownership fix.

The release build is frozen at artifacts/candidates/bridge-pending-retention-20261019-selected/war-legacy-bridge. build.json confirms recompilation and full source/binary hashes. The heap diagnostic exercises the frozen release artifacts; library tests and JS interoperability use debug builds of the same selected bridge source. All snapshots exclude the three existing dirty runtime experiments. Existing manual services and convenience executables are preserved.

Run `python docs/benchmarks/2026-10-19-bridge-pending-retention/verify.py` for offline source/profile/evidence checks. To repeat the diagnostic from the repository root, run `python docs/benchmarks/2026-10-19-bridge-pending-retention/probe.py --bridge FROZEN_BINARY --output FRESH.json`; it requires the recorded tcmalloc library and uses ephemeral loopback endpoints. Do not compare profiled timings or RSS as ordinary relay performance.
