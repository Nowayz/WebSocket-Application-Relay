# Bridge upstream welcome control handling, 2026-10-19

Source base `48dfd4b`. Selected fix: handle upstream WebSocket control frames while waiting for the application welcome. A single ten-second timeout wraps the entire wait. Ping is flushed (sending the automatic Pong), unsolicited Pong is ignored, Binary remains the welcome, and Close is acknowledged upstream and relayed downstream with its original optional status/reason. An early Close returns without inventing a legacy ID. The existing timeout also bounds this pre-welcome close exchange; control traffic does not reset it. No new queue, public option, payload-copy policy or post-welcome routing change is introduced.

Previously the first upstream message had to be Binary. A legal Ping/Pong or a native room-capacity rejection Close instead caused invalid-upstream-welcome failure and abrupt frontend transport loss. Two new real-socket library tests fail against that implementation: one cannot observe the early rejection Close, and the other encounters ResetWithoutClosingHandshake before receiving a welcome after upstream controls.

The rejection test handles both a policy code/reason and an empty Close payload. Two sequential clients reuse a bridge limited to one admitted connection, acknowledge Close, observe EOF and complete both upstream handshakes. The control test sends an unsolicited Pong and a Ping before welcome, verifies the exact Pong before releasing a uint64 welcome, then checks byte-for-byte legacy/CBOR private self-routing and clean close/EOF. Each test has a two-second outer safety bound and completes promptly. Neither performs a control-flood or full ten-second timeout-duration test; the unchanged single timeout is visible in source.

After the fix, all 104 clean-snapshot library tests pass in 0.51 seconds. Clippy with tests/warnings denied and formatting checks pass. The selected release bridge is freshly rebuilt (fresh:false) from that snapshot; source hashes match the archive. Pre-existing user overlays are excluded from the build and remain byte-identical.

The independent CLI check runs a real native relay with one member permitted per room, holds that member connected and makes two legacy joins through a bridge limited to one global and one per-IP connection. It then retires the holder and verifies that a replacement legacy client joins, receives an eight-byte ID, self-routes an exact ten-byte packet and closes normally.

| Actual full-room rejection | Original bridge | Selected bridge |
| --- | --- | --- |
| Frontend Close code | 1006 | 1013 |
| Frontend reason | empty | Try Again Later |
| Clean Close | false | true |
| Unwanted welcome/data messages | 0 | 0 |
| Denied attempts | 2 | 2 |
| Replacement admitted/routed | yes | yes |
| Native FD count before/after | 18 / 18 | 18 / 18 |
| Bridge FD count before/after | 10 / 10 | 10 / 10 |

Both CLI runs verify four accepted native TCP connections, two successful joins/retirements and two join rejections. Final connection/session/queued/variable/read-reservation/output-capacity gauges are zero, and both processes exit zero. The original case intentionally asserts the observed bad close behavior; passed=true there is a successful reproduction, not acceptance of the bug. Initial controller attempts did not launch the Node workload: inherited RUST_LOG=warn suppressed the info-level startup marker. The final driver sets RUST_LOG=info for its disposable children and uses fresh v2 paths; preliminary failed-startup receipts/logs are retained separately and do not represent relay failures.

The selected bridge also passes the full 61-test JavaScript/native/legacy suite in 1.111 seconds, including the unchanged original relay.js script and recent modern-client close/send-error checks. Its native peer is the existing idle-enabled JS fixture, separately pinned in that receipt. The CLI rejection checks use the more recent frozen native slot-gauge binary. Neither is a native performance comparison. All integration fixtures exit normally with zero tracked resources. No 100,000-user or throughput campaign is needed for this handshake-phase correction.

Original/selected sources, patch, fresh build identities, failing and passing test logs, exact CLI driver/Node checks and integration receipts are archived. Run python3 verify.py here for an offline audit with no live services. To rerun artifact drivers, copy them/tests to the repository artifacts directory, invoke from repository root and use fresh output paths plus the recorded frozen binaries. The selected executable is artifacts/candidates/bridge-welcome-selected/war-legacy-bridge. Existing manual bridge/native services and convenience binaries were not replaced or restarted; the source and isolated selected binary are verified, not promoted to those services.
