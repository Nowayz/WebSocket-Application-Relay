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
| Shutdown deadline | 10 s | Remaining tasks are aborted and registration cleanup runs |

Queue limits concern logical payload storage, not the complete process RSS. WebSocket
read/write buffers, partially assembled incoming frames, metadata, allocator
overhead, stacks, and kernel socket buffers also cost memory. In particular, a
large incoming-message limit multiplied by many simultaneous incomplete messages
can dominate memory; set it to what the application needs. The sample environment
uses 64 KiB rather than the compatibility-oriented 16 MiB default.

Output has a separate limit: a rewritten sender ID may expand an application
envelope by up to eight bytes, while channel-list replies can be larger than an
incoming application frame. Queue budgets must fit the input limit plus ten
header bytes; the WebSocket writer uses that output budget. Incoming limits still
apply independently to each client message.

Individual queue overflow closes the slow recipient with `1013`. Process-wide
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

## Scaling boundary

Workers share one process-local registry; private routing and variables do not
cross process boundaries. Multiple independent replicas need an explicit ownership
and routing scheme before applications can treat them as one relay. Measure
connection churn, active-frame sizes, fanout, queue pressure, p99 latency, CPU,
and memory on the intended TLS/network path before increasing capacity. The
initial measurements cover local single-host sockets, not a deployed cluster.
