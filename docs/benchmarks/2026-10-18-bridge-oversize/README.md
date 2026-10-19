# Legacy bridge oversized-frame Close handling

Selected: frontend WebSocket capacity failures now send Close 1009 (`Message Too Large`) before the bridge drops the connection, both before subscription and during ordinary relay service. The existing 16 MiB incoming message limit and bounded Close helper remain unchanged. Other errors retain their existing behavior. No dependency, queue, payload copy or new configuration was added.

Previously, a capacity failure before subscription propagated through `?`, while an active connection hit the catch-all stream termination branch. Both paths dropped TCP without sending a Close frame. Two independent raw-client cases reproduce EOF against the frozen prior bridge. The selected build returns an explicit 1009 Close in both cases.

Each case sends only a 14-byte masked WebSocket header advertising 16 MiB plus one byte; no oversized body is allocated or transmitted by the fixture. A separate native CBOR witness verifies an exact self-delivery FIFO barrier after each rejection. Final connections, sessions, queued bytes and variable bytes return to zero, and both disposable processes exit normally. This verifies the terminal decoder-error Close receipt, not a completed normal Close handshake after an invalid unfinished frame.

A durable Tokio regression test covers both bridge stages over actual TCP sockets, with a two-second outer deadline. All 91 clean-source library tests pass in 0.55 seconds, and Clippy passes all targets with warnings denied. The existing 34-test native/legacy JavaScript suite also passes in about 1.1 seconds, including unmodified original-client compatibility, private routing, command layouts, structured-value rejection, Close forwarding and idle policy. There is no throughput claim or long capacity run for this error-path correction.

The source patch applies to clean base 70dcd3d. The selected snapshot excludes the three protected working-tree overlays; build.json confirms actual recompilation and identifies the frozen executable and source hashes. The full sources, failed baseline and successful raw receipts, test logs and compatibility receipt are retained. Manual bridge/native services and convenience executables were not promoted or restarted.

Run `python docs/benchmarks/2026-10-18-bridge-oversize/verify.py` to audit the evidence without starting services. The archived raw fixture accepts an explicit bridge binary and fresh receipt path for reproducing the two cases locally.
