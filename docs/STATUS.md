# Continuing goal status

The user's ongoing goal is to port the upstream relay faithfully to Rust and keep
improving simplicity, code, documentation, stability, performance, concurrency,
and adjacent platform management. A later instruction makes CBOR the primary
protocol. The goal remains active; this file is a reviewable checkpoint, not a
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
- Current checks: 25 Rust tests and 6 JavaScript socket tests pass, along with
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

1. Repeat the sustained workload to assess the observed roughly 6% throughput
   difference after adding output cancellation. Profile before attributing it to
   watch/registry contention or selecting a faster representation. Keep the close
   and immediate-eviction contracts intact.
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
