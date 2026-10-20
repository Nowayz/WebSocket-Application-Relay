# Operating the native relay

`war-relay --help` lists every option; the [configuration reference](#configuration-reference)
below gives each one's environment variable, default and valid range. Secrets
are loaded from `WAR_AUTH_PASSWORD` and `WAR_ADMIN_TOKEN`; invalid encoding or
unusable HTTP admin tokens fail before binding without echoing secret values.
Empty values retain absent-secret behavior. Set configuration explicitly for
each deployment. Terms such as session, room, departure queue and staging
credit are defined in the [glossary](glossary.md).

## Listeners and ingress

The relay defaults to `127.0.0.1:1338` and management to `127.0.0.1:1339`.
Deploy behind a TLS WebSocket ingress; [Caddyfile.example](../deploy/Caddyfile.example)
shows the native route.

### Browser origin admission

`WAR_ALLOWED_ORIGINS` (or repeated `--allowed-origin`) lists exact browser
origins such as `https://app.example` or `http://localhost:4180`; the literal
`null` origin can be listed too. Up to 64 entries are accepted. With a list,
an upgrade whose `Origin` header is absent is still admitted, because native
clients send none; an `Origin` that is not listed, or a request with more than
one `Origin` header, receives HTTP `403` before the upgrade and counts in
`war_origin_rejected_total`. Without a list every origin is admitted. Origin
checking is separate from relay authentication and application authorization.

Accepted relay and management connections use `TCP_NODELAY` to disable Nagle.
The relay does not use `TCP_CORK`, `MSG_MORE` or a timed output batching window.
Private output attempts a socket write immediately; fanout wakes the owning
workers immediately and combines only output already queued. Bounded worker
quanta preserve FIFO and give other ready users a turn without a batching timer.
TCP flow/congestion control and the peer's receive window still govern progress.
Browser WebSocket implementations and TLS ingress own their separate socket
settings. Linux `TCP_QUICKACK` controls local ACK behavior temporarily, not the
peer's ACK policy or a permanent transmit delay switch; it is not reapplied on
every read. See [tcp(7)](https://man7.org/linux/man-pages/man7/tcp.7.html).

Linux also has a separate `net.ipv4.tcp_autocorking` policy, which coalesces small
writes while an earlier packet is still in the device/qdisc queue. On a dedicated
relay host or network namespace, `sysctl -w net.ipv4.tcp_autocorking=0` disables
that policy. This is a privileged, namespace-wide setting rather than a socket
option; the relay does not change shared-host sysctls at startup. See the
[kernel reference](https://www.kernel.org/doc/html/latest/networking/ip-sysctl.html).

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
headroom, a 262,144 descriptor limit, 64 KiB requested send buffers, 64 KiB
requested receive buffers, 16 MiB maximum encoded messages and 256 MiB global allowances
for output and decoder staging. The room ceiling is 1,024 members. These are
operating ceilings and safeguards, not preallocated memory or a universal
capacity promise. Large-room diagnostics raise that room ceiling explicitly.

Both supplied deployment profiles retain large-file support: a 5 MiB image can
be sent in one CBOR byte string, with its normal routing envelope. The encoded
message limit is 16 MiB and the per-session output ceiling is 32 MiB; neither is
preallocated for idle sockets. Keep large files as bytes instead of Base64 text.
Global decoder/output budgets still apply, so concurrent large transfers and
large multicast audiences must fit available credit. Payload bodies are shared
across recipients, while per-recipient pending delivery remains conservatively
charged. Use backpressure or reduce the offered rate when these budgets fill.
The [large-file check](reports/large-files.md) records the acceptance and socket-buffer
comparison behind this profile.

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

### Output queues and pressure

`WAR_QUEUE_MESSAGES` and `WAR_QUEUE_BYTES` are per-session allowances;
`WAR_TOTAL_QUEUE_BYTES` is the global budget shared by every session, in-flight
body and captured recipient view. `war_queued_bytes` reports global
reservations as logical credit, not RSS; a reservation is held until the write
completes or is cancelled. A session that exhausts its own allowance is closed
with `1013`; it is the slow consumer. When the global budget is exhausted, the
frame for the recipient being charged is dropped and `war_global_pressure_total`
increments. That recipient is closed with `1013` only if it already holds at
least a quarter of its own `WAR_QUEUE_BYTES`, or if the dropped frame was the
reply to its own command. Senders are never closed because of their recipients.
Delivery under pressure is best-effort: later recipients of one broadcast may
miss a frame that earlier recipients received.

Departure notices use one queue per room, plus one shared queue for `re_globl`
departures and disconnect-observer notices and one for broadcasts whose source
left mid-delivery. Notices to at most 128 recipients are delivered immediately
when the room's queue is idle. Larger ones become jobs that workers advance in
128-recipient turns, rotating across queues, so one large room cannot delay
another room's notices. Live broadcasts above 1,024 recipients advance in
1,024-recipient turns owned by the source. Each job reserves staging credit for
its captured view from the global budget; a refused reservation falls back to
synchronous delivery, in order, and counts in `war_global_pressure_total`, so
that counter records refused reservations, not lost messages. A departing
peer's normal Close does not prove that recipients got its notice; verify the
expected recipient matrix. The design is in [architecture.md](architecture.md).

## Lifecycle and deadlines

Terminal membership removal discards each room's cached recipient index instead
of maintaining it for future broadcasts. Captured fanouts keep their own owners;
reversible drain retains ordinary cache maintenance. The
[warmed-room shutdown check](reports/shutdown-views.md) covers a large room after real
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

Terminal shutdown suppresses departure traffic, clears the departure queues and
releases global listener indexes before retiring sessions, avoiding quadratic
notification and index-search work. It flushes `1001` before TCP retirement
where possible and does not wait for a peer acknowledgment during process
shutdown. A blocked partial frame can force an abnormal close instead. Ten
seconds is the default worker grace (`WAR_SHUTDOWN_TIMEOUT_SECS`); supervisor
cleanup gives workers one additional second to report. The systemd unit's stop
deadline must cover this process-level cleanup.

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
runs as failures. [Historical findings](reports/performance-history.md) explain selected
changes, rejected optimizations and their scope.

## Configuration reference

Every option is a CLI flag with an environment variable. Validation runs before
the relay binds; an out-of-range value is a startup error. Sizes are bytes.

| Flag | Environment | Default | Valid range | Meaning |
| --- | --- | --- | --- | --- |
| `--bind` | `WAR_BIND` | `127.0.0.1:1338` | Socket address | Relay WebSocket listener |
| `--admin-bind` | `WAR_ADMIN_BIND` | `127.0.0.1:1339` | Socket address; non-loopback requires `WAR_ADMIN_TOKEN` | Management HTTP listener |
| `--no-admin` | `WAR_NO_ADMIN` | off | Flag | Do not bind the management listener at all |
| `--listen-backlog` | `WAR_LISTEN_BACKLOG` | `1024` | 1 to 65535 | Requested pending TCP queue per worker listener; the OS may cap it |
| `--tcp-send-buffer-bytes` | `WAR_TCP_SEND_BUFFER_BYTES` | `0` | 0, or 4096 to 16777216 | Requested kernel send buffer per socket; 0 keeps the OS default |
| `--tcp-receive-buffer-bytes` | `WAR_TCP_RECEIVE_BUFFER_BYTES` | `0` | 0, or 4096 to 16777216 | Requested kernel receive buffer per socket; 0 keeps the OS default |
| `--allowed-origin` | `WAR_ALLOWED_ORIGINS` | unset (all origins) | Up to 64 entries of 1 to 1024 bytes, each an exact `http(s)://host[:port]` or `null`; repeat or comma-separate | Browser Origin allowlist; see [Browser origin admission](#browser-origin-admission) |
| `--max-connections` | `WAR_MAX_CONNECTIONS` | `10000` | 1 to 4294967295 | Admitted TCP connections per process, including handshakes |
| `--max-connections-per-ip` | `WAR_MAX_CONNECTIONS_PER_IP` | `0` | 0 to 4294967295 | Admitted connections per direct peer IP; 0 disables |
| `--max-channels` | `WAR_MAX_CHANNELS` | `10000` | 1 to 4294967295 | Channels, including `re_globl` |
| `--max-channel-members` | `WAR_MAX_CHANNEL_MEMBERS` | `10000` | 1 to 4294967295 | Sessions per channel |
| `--max-message-bytes` | `WAR_MAX_MESSAGE_BYTES` | `16777216` (16 MiB) | 17 to 67108864 (64 MiB) | Largest encoded message, including the routing envelope |
| `--queue-messages` | `WAR_QUEUE_MESSAGES` | `64` | 1 to 4294967295 | Queued frames per session before it is a slow consumer |
| `--queue-bytes` | `WAR_QUEUE_BYTES` | `33554432` (32 MiB) | At least `max-message-bytes` + 10 | Per-session output byte allowance |
| `--total-queue-bytes` | `WAR_TOTAL_QUEUE_BYTES` | `268435456` (256 MiB) | At least `queue-bytes` | Global output budget shared by all sessions, bodies and captured views |
| `--read-buffer-bytes` | `WAR_READ_BUFFER_BYTES` | `4096` | 1 to 1048576 | Base decoder receive storage per connection |
| `--total-read-buffer-bytes` | `WAR_TOTAL_READ_BUFFER_BYTES` | `1073741824` (1 GiB) | 0, or at least 2 × max(`read-buffer-bytes`, 20480) | Shared decoder staging allowance; 0 disables that admission check |
| `--read-buffer-idle-secs` | `WAR_READ_BUFFER_IDLE_SECS` | `5` | 0 to 86400 | Reclaim enlarged decoder storage after this long without large input; 0 disables |
| `--max-variables` | `WAR_MAX_VARIABLES` | `256` | 1 to 4294967295 | Variables per channel |
| `--max-variable-bytes` | `WAR_MAX_VARIABLE_BYTES` | `1048576` (1 MiB) | 1 to 4294967295 | Key plus value bytes per channel |
| `--total-variable-bytes` | `WAR_TOTAL_VARIABLE_BYTES` | `67108864` (64 MiB) | At least `max-variable-bytes` | Global channel variable budget |
| `--messages-per-second` | `WAR_MESSAGES_PER_SECOND` | `1000` | Any; 0 disables | Per-connection inbound message token bucket; exceeding it closes with `1008` |
| `--bytes-per-second` | `WAR_BYTES_PER_SECOND` | `16777216` | Any; 0 disables | Per-connection inbound byte token bucket; exceeding it closes with `1008` |
| `--handshake-timeout-secs` | `WAR_HANDSHAKE_TIMEOUT_SECS` | `5` | 1 to 86400 | Time allowed to complete the HTTP upgrade after accept |
| `--join-timeout-secs` | `WAR_JOIN_TIMEOUT_SECS` | `10` | 1 to 86400 | Time allowed after upgrade to send the channel name; expiry closes with `1008` |
| `--write-timeout-secs` | `WAR_WRITE_TIMEOUT_SECS` | `5` | 1 to 86400 | Retention deadline for a queued write |
| `--ping-interval-secs` | `WAR_PING_INTERVAL_SECS` | `15` | 0 to 86400 | Server heartbeat interval; 0 disables heartbeats |
| `--pong-timeout-secs` | `WAR_PONG_TIMEOUT_SECS` | `45` | 1 to 86400 | Missing-pong deadline; expiry closes with `1001` |
| `--idle-timeout-secs` | `WAR_IDLE_TIMEOUT_SECS` | `0` | 0 to 86400 | Close joined sessions with no application input or output; 0 disables |
| `--shutdown-timeout-secs` | `WAR_SHUTDOWN_TIMEOUT_SECS` | `10` | 1 to 86400 | Worker grace to send `1001` and retire sockets at terminal shutdown |
| `--disable-global-broadcast` | `WAR_DISABLE_GLOBAL_BROADCAST` | off | Flag | `re_globl` broadcasts require an authenticated sender |
| `--workers` | `WAR_WORKERS` | available CPUs, capped at 16 | At least 1 | Number of native I/O worker threads |
| `--pin-workers` | `WAR_PIN_WORKERS` | off | Flag; needs one allowed CPU per worker | Pin each worker thread to one CPU |

Two secrets are environment-only and never appear in `--help` or logs:

| Environment | Valid values | Meaning |
| --- | --- | --- |
| `WAR_AUTH_PASSWORD` | Up to 4096 bytes; empty or unset disables | Password for relay command `0`; enables listener flags, channel listing and ID assignment |
| `WAR_ADMIN_TOKEN` | 1 to 4096 bytes, a valid HTTP header value without trailing space or tab; empty or unset disables | Bearer token for privileged management routes; required for a non-loopback `--admin-bind` |

## Sizing memory

The three global budgets exist so the relay refuses work before the kernel
kills the process. Their sum, `WAR_TOTAL_READ_BUFFER_BYTES` plus
`WAR_TOTAL_QUEUE_BYTES` plus `WAR_TOTAL_VARIABLE_BYTES`, is the application
memory the relay may commit under load; at the defaults that is 1 GiB plus
256 MiB plus 64 MiB. Add kernel socket buffers, roughly the connection count
times the sum of the two TCP buffer sizes (requested or autotuned, and Linux
commonly doubles a request), plus the resident baseline of the process. That
total must fit under the cgroup memory limit with headroom, or the OOM killer
ends the process instead of the budgets refusing admission. Raise a budget
only together with the limit that contains it.

After a load test, check that the six gauges the release gate requires to be
zero have returned to zero: `war_connections`, `war_sessions`,
`war_queued_bytes`, `war_variable_bytes`, `war_read_buffer_reserved_bytes`
and `war_output_queue_capacity_bytes`. A gauge that stays non-zero after every
client has closed is a leak, not warm capacity.
