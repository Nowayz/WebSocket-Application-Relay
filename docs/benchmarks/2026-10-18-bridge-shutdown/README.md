# Short bridge shutdown lifecycle check

Verify the current bridge cancellation policy without changing relay code or adding a permanent CLI. SIGTERM cancels connection tasks and drops both TCP legs; active legacy clients can receive EOF/reset without a WebSocket Close. This is separate from the native relay's bounded WebSocket shutdown behavior. Local bridge replacement remains guarded by idle-client/resource checks.

Two fresh disposable native/bridge pairs use the selected frozen binaries. The native SHA-256 is `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`; bridge SHA-256 is `bb2392f53bd8fe1bdff47f0a8e790b366269400e259839565bdeb4573f9f8bf1`. A direct native witness self-routes checked 64-byte values continuously. Each bridge client joins a distinct room and verifies a 64-byte self echo. An equal population sends incomplete `GET /` HTTP prefixes, and an excess raw connection must be refused while the bridge's downstream cap is full.

| Joined legacy clients | Incomplete HTTP peers | Parent-observed successful bridge exit after SIGTERM | Verified native witness echoes during setup/cancellation |
| --- | ---: | ---: | ---: |
| 32 | 32 | 1.097 ms | 3,665 |
| 128 | 128 | 3.175 ms | 10,102 |

The exit observation includes parent scheduling/process-wait polling. It is not server-hop latency, a shutdown SLO, a saturation benchmark, or a 100,000-user guarantee. Each case is checked against a three-second process-exit guard, rather than waiting for the ten-second handshake timeout. Joined bridge sessions are idle at cancellation after their verified echo; arbitrary blocked-write/fragment/hostile-traffic cancellation is outside this fixture.

All joined legacy transports and partial-HTTP transports terminate through EOF/reset without additional application bytes. Their native upstream sessions retire, while the direct witness remains joined and routes an additional explicit post-shutdown probe. Native metrics progress from 33/129 connections and sessions to one, then zero after the witness's normal 1000 Close/EOF. Only the global channel remains. Queue/variable/read-buffer/output-capacity gauges and routing copied bytes return to zero. Application message, delivery, reused-value and byte counters match exactly. Both child processes exit successfully without forced termination.

The native `war_transport_disconnects_total` increases by exactly 32/128, matching cancelled upstream sessions. Unrelated protocol/handshake/write/close/pressure counters remain zero. These expected disconnects explain why stopping a bridge with live users should not be mistaken for a transparent upgrade. No native restart is needed; production defaults, application protocol and selected long-running services remain unchanged.

The initial probe used an incorrect 77-byte native envelope assumption for a 64-byte legacy payload. Its first byte-counter assertion failed by exactly one byte per legacy echo. The corrected counter derives canonical integer width and the byte-string header (two bytes for a 64-byte value), then checks final totals without weakening lifecycle checks. The failed receipt, error log and initial source remain archived; this was a fixture arithmetic correction, not a relay fix. The final fixture also waits for zero final resource gauges within its bounded gate, allowing asynchronous cleanup to settle.

## Reproduce and audit

Copy `probe.py` to `artifacts/bridge-shutdown-probe.py` in the repository; its imports resolve that directory's parent as the project root. Then use fresh output paths:

```sh
python3 artifacts/bridge-shutdown-probe.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --bridge artifacts/candidates/bridge-text-selected/war-legacy-bridge \
  --users 32 --output artifacts/bridge-shutdown-reproduction.json
```

Repeat with `--users 128` for the second population. These commands start and stop their own fresh processes on ephemeral ports. They never signal the selected long-running services. The probe fixes four native workers and four bridge runtime workers, disables fixture rates/heartbeats/application idle, and preserves full exact counters and source/binary/helper hashes. It uses existing independent raw wire helpers; no dependency or system setting is changed.

`python3 docs/benchmarks/2026-10-18-bridge-shutdown/verify.py` checks archive hashes, source/helper provenance, pre/post cancellation populations, exact native transport-disconnect and application counters, final resource retirement and witness continuity. Raw JSON is losslessly compressed. No network access or running server is needed for that audit. All three pre-existing user overlays remain byte-identical, and selected live native/bridge executable identities were rechecked unchanged.
