# Cancel bridge upstream setup when the frontend retires

Selected: frontend Ping/Pong and Close/EOF handling now runs while the upstream TCP/WebSocket connection future is pending, using the existing single setup-message slot. A frontend leaving while the upstream HTTP upgrade stalls no longer retains its bridge admission slot until the connection deadline. No new queue, task, setting, payload copy or dependency is introduced. Each existing connection/welcome timeout stays ten seconds; this is not a new total-setup latency guarantee.

Previously, after accepting a legacy channel name, bridge() awaited connect_async_with_config directly. An upstream could accept TCP and withhold HTTP 101. A frontend Close then got no prompt acknowledgement, and frontend EOF did not cancel that pending connection. Both reproduced with a global admission limit of one.

The selected code pins that same connection future and polls it alongside frontend input inside the unchanged timeout. Ping flushes its automatic Pong; Pong is ignored. Close is acknowledged and EOF retires the attempt. Returning drops the unfinished connection future and its owned TCP connection. One application message can be retained without copying; then frontend reads pause. The same pending slot carries into welcome and normal forwarding, preserving welcome-before-data and application FIFO. On a connection failure, retained input is dropped before bounded frontend Close cleanup. Existing TCP_NODELAY, decoder frame limits, successful subscription and conversion behavior remain unchanged.

## Focused evidence

- Two actual-socket regressions fail before the fix in a two-second suite. The upstream consumes the complete HTTP upgrade request and deliberately sends no response. Frontend Close fails the one-second acknowledgement guard; frontend EOF fails the two-second recovery guard. Both tests own their tasks through JoinSet, which aborts them on failed test cancellation.
- Both regressions pass in the selected full suite. Each confirms upstream TCP EOF, then connects through the same one-slot bridge to a working upstream, receives the exact little-endian ID 42, closes cleanly and joins owned tasks. The original long connection deadline is never shortened to make the tests pass.
- A shared startup fixture now checks both pre-welcome and pre-HTTP-upgrade stages, in binary and text modes. During the stalled phase it sends Ping and checks the exact Pong. It then pipelines two distinct messages, resumes the upstream, checks welcome ID 42 first, and checks exact CBOR/legacy conversions and FIFO through echo and clean Close. The earlier welcome-stage coverage remains.
- An independent [release socket probe](release-probe.py) runs the frozen executable with max-connections=1. A raw upstream accepts HTTP but withholds its response. Ping/Pong plus Close cancels the upstream in about 41 ms; EOF cancels it in about 0.05 ms in this single local observation. These are descriptive fixture timings, not a production latency percentile. Both cases restore descriptors to 10, and the bridge exits normally with status zero. The second connection also proves that admission was reusable. [Receipt](release.json).
- All 136 library tests pass in 10.02 seconds, including the existing ten-second welcome-timeout regression. Strict library/test Clippy passes in 2.12 seconds. All 65 JavaScript interoperability checks pass in 1.123 seconds; all six checked resource gauges retire to zero.

One initial JavaScript fixture omitted the repository-root original relay.js; 64/65 checks passed and the legacy-client check failed with ENOENT. That incomplete-fixture log is retained and excluded. Adding the unchanged tracked relay.js and rerunning the same checker produces the accepted 65/65 result. No service or runtime behavior was changed to address the fixture issue.

## Bounds and limits

This retains exactly one owned setup application message across both phases. After it is retained, further frontend reads pause. A Ping/Close behind that application message can therefore still wait through connection and welcome deadlines. This design preserves FIFO and bounded storage; it does not parse around application traffic or add an unbounded startup queue. Cancellation during an uncompleted HTTP upgrade is directly tested; no WAN/TLS, stalled DNS-worker retirement, throughput or 100,000-user performance claim is made.

Builds and checks used clean copied source snapshots at base 036ba19 with only src/legacy_bridge.rs changed. The three pre-existing dirty relay overlays were excluded. The selected frozen executable is artifacts/candidates/bridge-connect-20261019-selected/war-legacy-bridge, with [build/source hashes](build.json). Existing manually running services and convenience binaries were preserved; this change has not been promoted to those processes.

From the repository root, use a fresh output path for the release probe:

```sh
python docs/benchmarks/2026-10-19-bridge-connect-cancellation/release-probe.py \
  artifacts/candidates/bridge-connect-20261019-selected/war-legacy-bridge \
  artifacts/bridge-connect-fresh.json
```

The probe imports the existing scripts/reference.py raw wire helper; its hash is recorded. Startup and socket operations are bounded, owned server shutdown has a bounded kill/reap fallback, and receipt creation refuses an existing output. Run `python docs/benchmarks/2026-10-19-bridge-connect-cancellation/verify.py` for an offline audit. Compressed original/selected sources, the pre-fix test source, patch, logs and receipts are retained here. The runtime fix is the connection wait select loop and movement of the existing pending slot; most patch lines are actual-socket regression coverage.
