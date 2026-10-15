# Continuing goal status

The user's ongoing goal is to port the upstream relay faithfully to Rust and keep
improving simplicity, code, documentation, stability, performance, concurrency,
and adjacent platform management. A later instruction makes CBOR the primary
protocol, and the latest instruction emphasizes preserving the upstream's
zero-copy routing intent. The goal remains active; this file is a reviewable checkpoint, not a
claim that an open-ended improvement effort is complete.

## Implemented and locally exercised

- Upstream downloaded at `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`; source/history
  and MIT attribution retained on local branch `codex/rust-relay`.
- Rust/Tokio CBOR relay: channel and global broadcast, cross-channel private
  messages, listener flags, typed channel variables, entropy-backed IDs, fixed-ID
  takeover, and disconnect events.
- One CBOR routing envelope. Definite-length values are bounded and forwarded
  verbatim. Browser/Node-oriented JS client preserves uint64 IDs as BigInt and
  serializes control requests.
- Separate optional legacy bridge; original `relay.js` exercised unchanged, plus
  modern/legacy byte-string and text interoperability.
- Bounded handshake/join/admission, per-peer and process queue budgets, channel
  and process variable budgets, inbound rates, heartbeat, write and shutdown
  deadlines. RAII cleanup and takeover ownership checks prevent stale removal.
- Separate management listener with liveness/readiness, Prometheus, authenticated
  channel inspection, drain/resume, and administrative disconnection.
- Release socket workload, independent CBOR and legacy vectors, Rust network
  integration checks, client checks, CI workflow, protocol and operations docs.
- Current checks: 38 Rust tests and 6 JavaScript socket tests pass, along with
  strict Clippy and formatting. CI is supplied but has not run on GitHub.
- Read buffer changed from 128 KiB to 4 KiB after repeated 2,000-socket measurements
  showed about 91% lower idle RSS. Throughput variability is documented separately.
- Shutdown suppresses quadratic disconnect traffic; output limits allow channel
  queries larger than incoming application-message limits.
- Sustained checks found and fixed missing Close acknowledgements and delayed
  eviction of unread recipients. Shared close handling covers native CBOR and
  legacy clients, before and after join. Pending writes observe policy cancellation;
  routing membership/queued reservations release before a graceful handshake.
- A 30-second rerun verified 35,328,176 deliveries. A 20-second churn rerun completed
  1,911,994 connection lifecycles with 64 workers. An unread recipient was removed
  by the first 250 ms post-overflow sample while all 530,408 healthy deliveries
  were verified. All cases returned to zero sessions/connections/queued bytes and
  the persistent global channel. See stability.md for method and caveats.
- Repeated, affinity-separated socket measurements reproduced the cancellation
  overhead. Reusing pinned terminal waits recovered it: median 1,623,686 verified
  deliveries/s versus 1,384,284 for the fixed-close control, with 17.1% less server
  CPU per delivery. Close/eviction behavior remains intact. Idle state cost rose
  about 0.50 MiB at 2,000 sockets. See performance.md for raw evidence and limits.
- Native forwarding now shares received application storage through fanout and
  vectored socket output. The small sender prefix is separate, so varying CBOR
  integer widths require no body movement. Tiny views after enlarged input are
  compacted before publication; ordinary small messages share the normal slab.
- Final repeated 64 KiB traffic used 27.4% less server CPU per verified delivery
  with 4.6% higher median throughput. At 1 KiB, throughput ranges overlapped and
  CPU per delivery rose 2.1%. The copy counter stayed zero in all traffic runs;
  pointer checks cover queue and write ownership. Median idle RSS rose 0.86 MiB
  at 2,000 sockets. Raw evidence and limits are in performance.md.
- Final lifecycle checks verified 917,429 cumulative reconnects and 259,154
  healthy deliveries alongside an unread client, with complete cleanup. Finite
  fanout checks verified 31,500 deliveries per run. Fragmented input, interleaved
  controls, partial writes, and all WebSocket output length boundaries are covered.
- Local runtime refreshed on 2026-10-15: native CBOR at `127.0.0.1:1338`, legacy
  bridge at `127.0.0.1:1337`, management at `127.0.0.1:1339`. Both management
  health/readiness checks returned 200. Three socket checks against these running
  release processes exercised the unchanged original client, mixed CBOR/legacy
  values, and clean Close. A separate release workload verified 16 private
  deliveries. Gauges returned to zero afterward. Binary/process evidence is in
  `benchmarks/2026-10-15-zero-copy-live.json`; this is a timestamped observation.
- Sustained mixed-size fanout now verifies every reader's frames, bytes, ordering,
  disconnect events, and simultaneous graceful departure, with bounded templates,
  histograms, and acknowledgements. Optional unread and delayed readers exercise
  concurrent data and control pressure.
- A 128-client departure wave reproduced resets when a capacity signal cancelled
  output before its first poll. The connection now distinguishes untouched output
  from a possibly partial frame and preserves an already queued peer Close on
  the safe closing path. Three old-build runs failed; all three fixed runs passed.
- Final-build 30-second / 128-client mixed-size acceptance verified 3,440,304
  deliveries to 126 healthy readers beside one unread peer and a delayed reader.
  Peak sampled RSS was 13.96 MiB. Routed bytes were 99.784% reused; the existing
  small-view compaction policy accounted for the rest. A repeated 1 KiB cost guard
  recorded 1,582,701 versus 1,536,783 median deliveries/s with the same p99 bucket;
  no general performance gain is attributed to these correctness changes. See
  stability.md for the separate implementation stages, outcomes, and limits.
- The current local release processes were refreshed after that fix. A real Node
  client group completed 128 simultaneous clean closes with code 1000, and three
  additional live checks exercised the original client, native/legacy values, and
  Close before/after joining. Readiness/liveness returned 200 and gauges returned
  to zero. The timestamped binary/process snapshot is in
  `benchmarks/2026-10-15-graceful-close-live.json`.
- A further regression reproduced missing Close acknowledgement after input
  quota exhaustion. Both receive loops now handle a valid Close before rate
  accounting; the regression exercises this before and after joining. Other
  inbound traffic retains its limits.

## Exact verification boundaries

The original bundled C++ executable cannot run here: `libtbb.so.2`,
`libssl.so.1.1`, and `libcrypto.so.1.1` are absent. Compatibility evidence therefore
comes from source-derived known packets and the actual upstream JS client, not a
live C++ differential run. Native CBOR client tests use Node 24's real WebSocket;
browser bundler execution has not yet been checked.

Deployment examples are provided. The systemd unit was verified using the built
workspace executable substituted for its intended `/usr/local/bin/war-relay`
installation path. No system service or public endpoint has been installed.
Caddy is not installed here, so its example was not executed. Measurements are
local single-host plaintext sockets; no production or cluster claim is made.

## Next useful iterations

1. Profile the remaining small-message CPU cost before changing counters, queue
   representation, or read storage. Mixed-size fanout is now exercised at 128
   concurrent clients; extend adversarial retention checks around slab thresholds
   and repeated large unknown-target primers followed by tiny routed values.
   Preserve ordinary zero-copy forwarding, compaction of enlarged slabs, exact
   peer Close acknowledgements, and immediate dropping of partial output.
2. Extend duration, fanout, active populations, adverse networks, and connection
   mixes. Thirty-second traffic and twenty-second churn are now verified, while
   multi-hour, TLS, and much larger concurrent populations remain unmeasured.
   Examine global counter/registry contention only if those profiles show it;
   keep the current simple ownership model until a measured bottleneck justifies
   a different representation or sharding.
3. Add browser bundler acceptance and deliberate malformed-CBOR/fragmentation
   campaigns, preserving resource bounds and source fidelity.
4. Revisit connection-admission policy for public multi-application operation
   (per-IP limits, origin policy, tenant boundaries) with an explicit compatibility
   and proxy model. The existing global/private scopes intentionally match WAR.
5. Design node ownership and cross-node routing only after single-node capacity
   and operational needs are established. No replicas are currently a cluster.
