# Clean Close for legacy channel-join timeout

Selected: when the existing ten-second frontend join wait expires, the bridge sends WebSocket Close 1008 / Channel Join Timeout using its existing bounded Close helper. This matches the native relay's code/reason for its corresponding policy. Previously the timeout propagated through .await?? and dropped TCP without a WebSocket Close, exposed as ResetWithoutClosingHandshake (typically code 1006 to a browser).

The runtime change handles the outer join timeout explicitly. Successful joins and inner decoder/transport results keep their existing behavior. No timer value, setting, task, queue, forwarding path, payload copy or dependency is added. The original elapsed error remains the connection's diagnostic result after cleanup, now with channel-join context.

## Actual-socket regression

The test starts a real bridge with global admission limited to one and opens an idle frontend WebSocket without a channel message. A real fake-upstream listener records whether any TCP connection was opened. The same test fails against the prior runtime after 10.00 seconds with ResetWithoutClosingHandshake; before.log and the pre-fix test source are retained.

On the selected runtime the client receives exact Close code 1008 and reason Channel Join Timeout at the unchanged join deadline. It acknowledges and retires the connection. No upstream connection has opened for the unjoined client. A second frontend then reuses the only slot, sends its channel, receives exact little-endian welcome ID 42 from the working upstream and closes cleanly. All owned tasks are joined; the outer fixture guard is 14 seconds. A lower elapsed guard prevents replacing the production deadline with a short test-only value.

Validation against clean copied selected sources:

- All 137 library tests pass in 10.03 seconds. The new real deadline test runs concurrently with the existing ten-second welcome timeout regression, keeping total suite duration about ten seconds.
- Strict library/test Clippy passes in 2.20 seconds.
- All 65 JavaScript interoperability checks pass in 1.119 seconds, with all six checked resource gauges returning to zero.
- The release bridge was freshly compiled and frozen with source and executable hashes in build.json. The new timeout behavior was exercised by the actual-socket library regression; no separate ten-second release wire run or 100k traffic test was repeated for this pre-join control-path change.

## Lifecycle budget and limits

The ten-second join deadline is unchanged. Unlike the previous abrupt TCP drop, a graceful Close exchange can retain admission for the existing additional ten-second Close budget when the peer withholds acknowledgement. The tested recovery path acknowledges; the unacknowledged path uses the unchanged common timeout helper. This is a set of phase budgets, not a hard end-to-end latency guarantee. README documents both budgets. No throughput or large-population performance improvement is claimed.

The connection-failure pending-owner path from the previous change was also inspected and already drops its retained message before Close cleanup; no additional lifetime change was needed there.

## Provenance and reproduction

Base: ddf3b10. Fresh copied Cargo sources excluded the three pre-existing dirty runtime overlays. Only src/legacy_bridge.rs differs for this change; compressed base/selected source, patch, failed-before source, test/check logs and the frozen release build receipt are archived. The new executable is artifacts/candidates/bridge-join-timeout-20261019-selected/war-legacy-bridge. Existing manually running services, convenience binaries and unrelated edits were preserved; no process promotion occurred.

From the repository root, the focused regression can be run against a clean selected snapshot:

```sh
cargo test --locked \
  --manifest-path artifacts/bridge-join-timeout-20261019-source/Cargo.toml \
  --target-dir artifacts/bridge-buffer-check --lib \
  frontend_join_timeout_closes_cleanly_and_recovers_the_only_slot
```

Run builds/checks serially and refresh copied source mtimes before reusing a Cargo cache. `python docs/benchmarks/2026-10-19-bridge-join-timeout/verify.py` audits saved evidence without waiting through deadlines or starting services.
