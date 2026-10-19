# Operating the native relay

`war-relay --help` is the configuration reference. Secrets are loaded from
`WAR_AUTH_PASSWORD` and `WAR_ADMIN_TOKEN`; invalid encoding or unusable HTTP
admin tokens fail before binding without echoing secret values. Empty values
retain absent-secret behavior. Set configuration explicitly for each deployment.

## Listeners and ingress

The relay defaults to `127.0.0.1:1338` and management to `127.0.0.1:1339`.
Deploy behind a TLS WebSocket ingress; [Caddyfile.example](../deploy/Caddyfile.example)
shows the native route. Use exact browser Origins with `WAR_ALLOWED_ORIGINS`.
Absent Origin remains allowed for native clients; Origin checking is separate
from relay authentication and application authorization.

Health and readiness are `/healthz` and `/readyz`. Management provides `/metrics`,
`GET /v1/channels?limit=...&after=...`, `POST /v1/drain`, `POST /v1/resume` and
`POST /v1/sessions/{id}/disconnect`. Use decimal uint64 IDs without JavaScript
rounding. Privileged routes require the configured bearer token. With no token,
metrics remain readable on the default local listener; health/readiness are
public. An external management bind requires a token. Keep management private.

`WAR_AUTH_PASSWORD` enables relay protocol authentication, global listener flags,
channel queries and fixed-ID assignment. It does not turn channels into access
control boundaries. Variable writes remain available to channel members. Private
IDs route across channels. Applications provide their own identity and permission
model. `WAR_DISABLE_GLOBAL_BROADCAST=true` requires authentication for global
sends without changing listener authorization.

## 100,000 native connections

Use the [100k environment](../deploy/war-relay-100k.env.example) and
[systemd override](../deploy/war-relay-100k.conf.example) as a starting profile.
The profile includes 110,000 physical connection slots for joining/closing
headroom, a 262,144 descriptor limit, 16 KiB requested send buffers, 64 KiB
requested receive buffers, 64 KiB maximum messages and 256 MiB global allowances
for output and decoder staging. The room ceiling is 1,024 members. These are
operating ceilings and safeguards, not preallocated memory or a universal
capacity promise. Large-room diagnostics raise that room ceiling explicitly.

The base [systemd unit](../deploy/war-relay.service) has lower limits and is not
sufficient for 100,000 sockets without its override. Include cgroup-charged kernel
socket memory, application storage and file descriptors in capacity planning.
Linux commonly reports doubled socket-buffer requests; check effective settings.
Small TCP windows can limit throughput on high bandwidth-delay networks.

Choose worker count and optional `WAR_PIN_WORKERS` for the actual host. Keep load
generators on different physical cores during comparisons, and record SMT
siblings, worker affinity and CPU topology. The fixed masks in mixed-load scripts
assume the original 32-logical-CPU test host; adjust them before another host.
Multiple relay processes do not share routing state. SO_REUSEPORT is used between
workers of one relay, not as a distributed membership mechanism.

## Memory and backpressure

Idle native sockets own no task, message channel or private receive slab. Workers
reuse base receive storage. Partial frames retain admitted storage; published
payloads retain immutable owners until their consumers finish. Oversized completed
worker slabs retire before reuse. `WAR_READ_BUFFER_IDLE_SECS` reclaims quiet
high-water decoder/fragment storage; incomplete messages and published payloads
keep their required ownership.

`WAR_TOTAL_READ_BUFFER_BYTES` bounds staging allowances. Growth reserves headroom
for overlapping old/new allocations before copying unfinished input. A zero
configured staging allowance disables that safeguard; do so only with an
articulated alternative resource plan. Codec provenance and exact Rust patches
are in [vendor/README.md](../vendor/README.md).

`WAR_QUEUE_MESSAGES`, `WAR_QUEUE_BYTES` and `WAR_TOTAL_QUEUE_BYTES` govern retained
output. Reservations persist across partial writes and are released on completion
or cancellation. They include fanout staging and membership views. Logical
`war_queued_bytes` is not RSS; allocator/container capacity and kernel TCP buffers
are separate. A recipient that cannot accept bounded output is closed with
`1013`. Shared credit exhaustion refuses further reservations rather than growing
unbounded queues. Broadcasts and departure events are best-effort under pressure;
some recipients may have received output before a later reservation fails.

### Queue headroom for departure work

A 100,000-recipient view needs about 800,000 bytes of handles on a 64-bit host,
plus capacity and job/frame overhead. Ordinary room views remain conservatively
charged per active publisher/job. Disconnect-observer jobs share one charged
immutable view while membership is unchanged. A membership change creates a
separately charged generation if older jobs still retain their view. Allow for
concurrent generations, individual job metadata and output as well as payloads.
A constrained 256 KiB queue cannot stage such a view: synchronous fallback can
occur, and exhausted credit can drop notices. That diagnostic is outside the
normal profile's 1,024-member ceiling and does not show that the profile fails.

Watch queue occupancy and changes in `war_global_pressure_total`. The pressure
counter can include refused metadata even when fallback delivery succeeds; it is
not a missing-message count. Verify the expected recipient matrix. A departing
source's normal Close does not prove its recipients received departure events.
The [snapshot ownership report](retirement-snapshots.md) checks an eight-departure
burst against 100,000 observers with the same constrained shared credit.

Ordinary room/departure work advances in 1,024-recipient turns. Clearing a global
listener's disconnect flag orders captured notices before its acknowledgment.
The barrier submits only that observer's still-pending notices and records a
sequence watermark so deferred jobs cannot replay them after re-subscription.
Other recipients retain their bounded turns. The barrier still scans pending
global recipient views under the registry lock; many such views can make the
control turn longer. Queue-budget fallback is also synchronous. These are known
limits, not fixed control-turn guarantees. See the
[observer-barrier report](observer-barriers.md).

## Lifecycle and deadlines

Terminal membership removal discards each room's cached recipient index instead
of maintaining it for future broadcasts. Captured fanouts keep their own owners;
reversible drain retains ordinary cache maintenance. The
[warmed-room shutdown check](shutdown-views.md) covers a large room after real
delivery, all shutdown Close frames and transport EOFs.

Handshake, join, write, heartbeat, application-idle and shutdown deadlines are
independent controls. `WAR_IDLE_TIMEOUT_SECS=0` preserves application-idle sessions.
Heartbeat interval zero disables server heartbeat generation. Tune idle policies
for your application; input/output activity and deadline semantics are covered by
native tests. Queued writes keep their own retention deadline.

Manual drain stops admission but allows existing sessions to run. Readiness
returns 503 while drained. An authenticated resume reverses a manual drain.
SIGINT/SIGTERM, worker failure and embedding cancellation start terminal shutdown;
management cannot reopen it. `Relay::set_draining(false)` reports failure after
terminal stopping. Construct a new Relay when restarting an embedded service.

Terminal shutdown suppresses departure traffic and releases global listener
indexes before retiring sessions, avoiding quadratic notification and index-search
work. It flushes `1001` before TCP retirement where possible and does not wait for
a peer acknowledgment during process shutdown. A blocked partial frame can force
an abnormal close instead. Ten seconds is the unchanged default worker grace;
supervisor cleanup gives workers one additional second to report. The systemd
unit's stop deadline must cover this process-level cleanup.

A strict 100,000-global-listener check verified every normal Close with the default
grace. A shortened two-second diagnostic verified only 72,846 normal Close frames
even though the server exited successfully. Size grace for population cleanup;
process exit alone does not prove all clients received a normal Close.

The embedding API is `Relay::new(config)` plus `serve(listener, admin, relay,
shutdown)`. Configuration loading/validation is fallible and should be propagated.
The runtime joins workers on shutdown and partial startup failure. Socket lifetime
retains physical admission while in-flight routes hold output owners, including
after routing membership is removed.

## Monitoring and checks

Track connections/sessions, accepted/rejected/closed counts, queued bytes,
decoder reservations, output queue capacity, native arena slots, variable bytes,
write/close errors, slow consumers, pressure and routing-copy counters. A zero
routing-copy counter describes application routing; it does not imply zero
kernel copying or zero first-use allocation. RSS can remain above initial values
after logical resources reach zero because allocators keep pages and small
arenas/queues stay warm for reconnects.

Use [short recipes](performance-guide.md) to check the workload on its intended
host. Test normal Close, EOF, exact payloads/FIFO and final resource release.
Do not overlap compilation, profiling or multiple timed generators. Record failed
runs as failures. [Historical findings](performance-history.md) explain selected
changes, rejected optimizations and their scope.
