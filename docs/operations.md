# Running and managing a WAR relay

## Process layout and TLS

Use the Rust binary as one long-lived relay process. Bind the WebSocket and admin
listeners independently. Public `wss://` uses a TLS reverse proxy; direct in-process
TLS is not implemented. The included [Caddy example](../deploy/Caddyfile.example)
forwards only the WebSocket listener and leaves management private. Caddy handles
WebSocket upgrades automatically; see its [reverse-proxy documentation](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy).

The [systemd unit](../deploy/war-relay.service) and [environment example](../deploy/war-relay.env.example)
show a 5,000-connection profile with 64 KiB incoming messages. They are deployment
examples, not a measured capacity guarantee. Install the release binary at
`/usr/local/bin/war-relay`, copy/edit the environment file to `/etc/war-relay.env`,
and install the unit into `/etc/systemd/system/`. Secret values belong in the
environment file with permissions restricted to its administrator.

The optional legacy bridge is a separate process and owns two sockets per client.
Keep it on a trusted network or behind TLS termination as well. Its upstream URL
currently supports plain `ws://`; connect locally to the relay. Native CBOR clients
avoid the translation, second socket, and extra memory/CPU entirely.

## Important defaults and budgets

`war-relay --help` is the source of truth for CLI options and matching `WAR_*`
variables. All bounds are per process.

| Budget | Default | Counts |
| --- | ---: | --- |
| Connections | 10,000 | TCP handshakes, unjoined sockets, joined sessions, and closing transports |
| Channels | 10,000 | Includes persistent `re_globl` |
| Members per channel | 10,000 | Includes closing sessions until removal |
| Incoming message | 16 MiB | Complete reassembled WebSocket data message |
| Read buffer per socket | 4 KiB | Initial read capacity; tune for the actual workload |
| Output queue per session | 64 messages / 32 MiB | Includes an in-flight socket write |
| Output reservations across process | 256 MiB | Logical payload bytes per recipient, conservatively counting shared frames repeatedly |
| Variables per channel | 256 / 1 MiB | Key bytes plus encoded CBOR value bytes |
| Variables across process | 64 MiB | Sum of channel key/value bytes |
| Incoming rate per session | 1,000 messages/s / 16 MiB/s | One-second burst; join and ping/pong also count |
| Handshake / channel join deadline | 5 s / 10 s | Includes clients that send no data |
| Write / graceful-close deadline | 5 s | One deadline per operation; close-send and acknowledgement share a budget |
| Heartbeat | Every 15 s, pong due within 45 s | Server ping payload `war`; zero interval disables heartbeats |
| Application idle timeout | Disabled (`0`) | Joined sessions; `--idle-timeout-secs` / `WAR_IDLE_TIMEOUT_SECS`, up to one day |
| Shutdown deadline | 10 s | Remaining tasks are aborted and registration cleanup runs |

Queue limits concern logical payload storage, not the complete process RSS. WebSocket
read/write buffers, partially assembled incoming frames, metadata, allocator
overhead, stacks, and kernel socket buffers also cost memory. In particular, a
large incoming-message limit multiplied by many simultaneous incomplete messages
can dominate memory; set it to what the application needs. The sample environment
uses 64 KiB rather than the compatibility-oriented 16 MiB default.

Routed output borrows immutable receive storage. A small slice can retain its
backing slab, so each connection tracks its largest observed message (at least
its configured initial read buffer). Once that mark exceeds the initial read
buffer, shared messages smaller than one eighth of the mark have their encoded
application value compacted before fanout. Ordinary tiny messages still share
the normal read slab. This conservative policy can continue copying small
messages after one large message; it avoids retaining enlarged slabs through
tiny queued views.
Queue gauges count the emitted CBOR envelope's logical bytes, including its
separate routing prefix, rather than the backing allocation's capacity. Measure
RSS alongside these gauges; they are not a physical-memory accounting interface.

Receive capacity can also remain resident on open connections after their output
queues drain. Concurrent 4 MiB primer measurements demonstrate this separately
from queued-view retention; the 4 KiB read setting is an initial capacity. Budget
connections and incoming message size together, and monitor process RSS as well
as logical queue bytes.

For applications that should retire inactive sessions, set
`WAR_IDLE_TIMEOUT_SECS=300` (or another value in `1..=86400`). The default `0`
preserves indefinite application-idle sessions. A completed binary input or a
successfully written relay message refreshes activity, including commands and
replies. Receive-only clients remain active while output arrives. Ping/Pong
traffic does not refresh application activity, and an unfinished fragmented data
message does not qualify as completed input. Queued/in-flight output is active
work governed by the existing queue and write limits.

An application-idle decision sends `1001` / `Application Idle` and increments
`war_idle_timeouts_total`. Routing membership and channel-variable ownership
release before the close handshake; TCP capacity and decoder storage release
when the transport ends. An unresponsive peer remains subject to the close
deadline. This policy retires inactive sessions; input-size/admission limits and
RSS monitoring still govern active populations. It shares one reusable deadline
wait with heartbeat timeout handling and leaves application-body sharing intact.

Output has a separate limit: a rewritten sender ID may expand an application
envelope by up to eight bytes, while channel-list replies can be larger than an
incoming application frame. Queue budgets must fit the input limit plus ten
header bytes; the WebSocket writer uses that output budget. Incoming limits still
apply independently to each client message.

When `--max-message-bytes` is smaller than 125, valid WebSocket control frames
still have their [RFC 6455](https://www.rfc-editor.org/rfc/rfc6455.html#section-5.5)
limit of 125 payload bytes. Complete and fragmented
application data retain the configured message limit. Ping/Pong still consume
input rate quotas; a valid Close remains exempt so cleanup can complete.

Individual queue overflow requests recipient closure with `1013`. An already
queued peer Close wins a simultaneous capacity decision when no partial output
exists. Process-wide
queue pressure refuses further reservations and closes the producing session with
`1013` instead of closing arbitrary unrelated recipients. A broadcast may already
have reached some recipients when global capacity runs out; there are no delivery
acknowledgements or transactional broadcasts. Disconnect notifications may be
dropped under global queue pressure. During full service shutdown they are
suppressed to avoid quadratic traffic among sessions that are all closing.

A close decision removes routing membership and releases queued reservations
before waiting for a graceful handshake. TCP connection capacity remains charged
until the transport ends. Policy cancellation interrupts an in-flight socket
write immediately. A partial or failed frame is never followed by a second
blocking flush; that transport is dropped, so an unread client may observe a
reset/abnormal close instead of the intended WebSocket close code. Responsive
clients receive and acknowledge the normal close frame. A received Close is
acknowledged by flushing the exact response already queued by the WebSocket library.
If cancellation arrives before the output future's first poll, that operation
has performed no I/O. Its stream can finish a normal close handshake. An inline
boolean distinguishes this case; the application body remains shared throughout.
An untouched stream may remain until the configured close deadline while waiting
for acknowledgement; its routing registration and queued bytes are already released.
Valid peer Close frames bypass input rate accounting before and after channel
join, so an exhausted quota cannot prevent cleanup. Data, joins, Ping, and Pong
retain their configured rate limits.

Global variable exhaustion refuses the write and closes the writer with `1013`.
Per-channel and process variable budgets update atomically with registry changes.
An empty ordinary channel releases its variables immediately. Global-channel
variables survive empty membership and remain charged until process restart.

## Trust model

The relay is a message transport, not an application authorization system.
Channel names are discovery/routing labels, not secrets. Any connected client may
write its channel's variables, and private IDs route across channels. The original
global channel intentionally has relay-wide reach and privileged listener access.
Use a connection-admission proxy or separate processes for applications with
different trust boundaries. `--disable-global-broadcast` requires authentication
for global-channel broadcasts.

`WAR_AUTH_PASSWORD` enables fixed-ID takeover, listener flags, and channel queries.
`WAR_ADMIN_TOKEN` separately enables management operations. No password grants
privileges when its environment variable is absent. Operators should use a private
admin listener or an authenticated TLS/allowlisted proxy. A token is required to
bind admin HTTP to a non-loopback address; health/readiness remain intentionally
public on that listener.

## Drain and restart

Use bearer authorization for management requests. In examples, `$WAR_ADMIN_TOKEN`
is a shell variable supplied by the operator; it is not a literal token.

```sh
curl --fail -X POST -H "Authorization: Bearer $WAR_ADMIN_TOKEN" http://127.0.0.1:1339/v1/drain
curl --fail http://127.0.0.1:1339/healthz
curl -i http://127.0.0.1:1339/readyz
curl --fail -H "Authorization: Bearer $WAR_ADMIN_TOKEN" http://127.0.0.1:1339/metrics
```

Draining preserves existing connections and rejects new connections/joins.
Readiness becomes `503`; liveness stays `200`. Either wait for sessions to leave,
send SIGTERM for bounded closure, or POST `/v1/resume` to return to service.
An application host can restart and reclaim its fixed ID without restarting the
relay or unrelated sessions. Restarting the relay itself ends its sessions and
loses in-memory variables; clients must reconnect and rebuild application state.

Monitor `war_connections`, `war_sessions`, `war_channels`, `war_queued_bytes`,
`war_variable_bytes`, `war_slow_consumers_total`, `war_global_pressure_total`,
`war_handshake_errors_total`, `war_protocol_errors_total`, and
`war_rate_limited_total`. Metric labels deliberately exclude user/channel IDs.
Debug logging reports connection failures without message bodies or credentials.

Output diagnostics distinguish `war_write_timeouts_total`, `war_write_errors_total`,
`war_aborted_writes_total` (policy or shutdown cancellation), and
`war_close_failures_total`. Abrupt TCP loss is counted in
`war_transport_disconnects_total`, rather than presenting an ordinary lost client
as a malformed WAR packet. During clean reconnect churn these counters should
stay at zero; forced unread-client eviction can increment aborted writes.

`war_unstarted_writes_cancelled_total` is the subset of aborted output operations
whose write futures were never polled. Those streams can close gracefully.
`war_slow_consumers_total` counts recipient capacity-close signals, which can
race an already queued peer Close during mass departure. A notification burst
can therefore increase it even when every departing client completes a clean
handshake. Examine close failures, transport disconnects, and the before-start
counter alongside capacity signals rather than equating each signal with a reset.

`war_routing_reused_bytes_total` and `war_routing_copied_bytes_total` count encoded
application value bytes once per routed input, before recipient fanout. The
latter measures deliberate slab compaction. They exclude relay command replies,
unknown destinations, WebSocket unmasking/reassembly, client serialization, and
kernel copies. Native application output writes its small headers and body as
separate slices, without a contiguous WebSocket payload staging buffer.

## Scaling boundary

Workers share one process-local registry; private routing and variables do not
cross process boundaries. Multiple independent replicas need an explicit ownership
and routing scheme before applications can treat them as one relay. Measure
connection churn, active-frame sizes, fanout, queue pressure, p99 latency, CPU,
and memory on the intended TLS/network path before increasing capacity. The
initial measurements cover local single-host sockets, not a deployed cluster.
