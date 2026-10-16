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

## Browser-origin admission

Both binaries accept `--allowed-origin` and `WAR_ALLOWED_ORIGINS`. The default
empty list accepts all origins, preserving existing clients. For example:

```sh
WAR_ALLOWED_ORIGINS=https://app.example,https://admin.example ./target/release/war-relay
./target/release/war-legacy-bridge --allowed-origin https://app.example --allowed-origin https://admin.example
```

Use each application's serialized `location.origin`: `http://` or `https://`,
host, and a nondefault port if present. This is the page origin, rather than the
relay's `ws://` or `wss://` URL. Comparisons are exact and case-sensitive; there
is no path, trailing slash, wildcard, suffix/subdomain matching or normalization.
Use the lowercase, ASCII/IDNA host and default-port omission supplied by the
browser. Configuration accepts at most 64 entries of up to 1,024 bytes each.

When a list is set, an unlisted Origin, an empty header, a combined origin list
or duplicate Origin headers receives HTTP `403` / `Origin Not Allowed` before
WebSocket upgrade and channel registration. The literal opaque origin `null`
is denied unless explicitly listed. Sandboxed/file/privacy contexts can share
that value; allowing it does not identify a particular page. A missing Origin
remains allowed for native clients and local bridge upstream connections.

Origin protects the browser scripting boundary described by
[RFC 6455 section 10.2](https://www.rfc-editor.org/rfc/rfc6455.html#section-10.2);
non-browser clients can omit or forge it. It does not authorize channels,
replace authentication, or change the relay's global/private routing scopes.
Use TLS and an admission/authentication proxy where client identity is needed.
Preserve the browser's Origin through that proxy. Removing or rewriting it to an
allowed value bypasses this check.

Configure every exposed ingress. In particular, setting only the native relay's
policy does not restrict the legacy bridge's browser clients: the bridge's
upstream is an Origin-less native connection. Set the policy on the bridge too.
The existing public `legacy_bridge::serve` library entry point remains
unrestricted; `serve_with_origin_policy` opts into the same policy as the CLI.
Both listeners cap raw HTTP upgrade input at 16 KiB. The bridge uses its existing
10-second handshake deadline and connection cap; it has no shared decoder budget
or management listener. That HTTP cap is removed after upgrade, so subsequent
WebSocket data still has the independent 16 MiB legacy message limit.

`war_origin_rejected_total` counts native policy denials once and also contributes
to `war_rejected_total`. A successfully written HTTP `403` does not increment
`war_handshake_errors_total` or `war_join_rejected_total`; a failed/timed-out
denial write still counts as a handshake failure. The bridge has no such metric:
its denials are visible as HTTP `403`, with connection errors at `RUST_LOG=debug`.
No Origin values are used as metric labels. Checks borrow bounded configuration
and header bytes once at upgrade; application messages retain their shared-body
forwarding path.

## Important defaults and budgets

`war-relay --help` is the source of truth for CLI options and matching `WAR_*`
variables. All bounds are per process.

| Budget | Default | Counts |
| --- | ---: | --- |
| Connections | 10,000 | TCP handshakes, unjoined sockets, joined sessions, and closing transports |
| Channels | 10,000 | Includes persistent `re_globl` |
| Members per channel | 10,000 | Includes closing sessions until removal |
| Incoming message | 16 MiB | Complete reassembled WebSocket data message |
| Worker receive chunk | 4 KiB | One reusable base slab per worker; idle sockets allocate none; incomplete frames retain separate storage |
| Decoder staging across process | 1 GiB | `--total-read-buffer-bytes` / `WAR_TOTAL_READ_BUFFER_BYTES`; twice frame/fragment capacity as growth headroom; zero disables |
| HTTP upgrade input | 16 KiB | Raw bytes read before upgrade completes; handshake timeout still applies |
| Private decoder/fragment high-water retirement | 5 s after large input | `--read-buffer-idle-secs` / `WAR_READ_BUFFER_IDLE_SECS`; zero disables this timer; oversized worker slabs always retire before reuse |
| Output queue per session | 64 messages / 32 MiB | Includes an in-flight socket write |
| Output reservations across process | 256 MiB | Logical payload bytes per recipient, conservatively counting shared frames repeatedly |
| Variables per channel | 256 / 1 MiB | Key bytes plus encoded CBOR value bytes |
| Variables across process | 64 MiB | Sum of channel key/value bytes |
| Incoming rate per session | 1,000 messages/s / 16 MiB/s | One-second burst; join and ping/pong also count |
| Handshake / channel join deadline | 5 s / 10 s | Includes clients that send no data |
| Write / graceful-close deadline | 5 s | Queued application output has a deadline from retention; close-send and acknowledgement share one budget |
| Heartbeat | Every 15 s, pong due within 45 s | Server ping payload `war`; zero interval disables heartbeats |
| Application idle timeout | Disabled (`0`) | Joined sessions; `--idle-timeout-secs` / `WAR_IDLE_TIMEOUT_SECS`, up to one day |
| Shutdown deadline | 10 s | Workers drop remaining transports and run registration cleanup |

Queue limits concern logical payload storage, not the complete process RSS. WebSocket
read/write buffers, partially assembled incoming frames, metadata, allocator
overhead, stacks, and kernel socket buffers also cost memory. In particular, a
large incoming-message limit multiplied by many simultaneous incomplete messages
can dominate memory. Decoder admission below bounds staging requests, while the
message limit should still match the application. The sample environment
uses 64 KiB rather than the compatibility-oriented 16 MiB default.

Before accepting a native WebSocket handshake, the relay reserves twice the greater
of the configured initial read capacity and 20 KiB (the 16 KiB raw HTTP bound plus
the HTTP reader's 4 KiB chunk). An exhausted shared staging allowance returns
HTTP `503`. After upgrade an empty decoder releases its HTTP reservation. Worker
receive storage and private partial-frame/fragment leases reserve
twice their requested capacities **before allocation**, covering overlap of old
and new staging allocations during growth. Large frame allocations include one
configured read chunk of bounded read-ahead so the next header can reuse storage;
that capacity is also charged. Frame growth is admitted before reading its
advertised body; fragment assembly is admitted before appending a decoded frame.
Denial stops input and sends a best-effort `1013` / `Input Memory Limit`.
A refused incomplete frame cannot be resumed unreserved.

The receive allocation and its lease move together between worker and decoder.
Storage moves only between frames with no unread bytes; a parsed header awaiting
its body also stays with its connection. Idle decoders keep a zero-byte lease.
A worker discards an oversized empty slab before lending storage again, and
releases scratch storage when its last socket retires. A small message from
another connection therefore cannot pin a previous large receive allocation.

Each retained lease keeps its high-water allowance, so normal reads within it perform
no shared-budget atomics or per-packet guard allocation. The allowance releases
on transport drop/cancellation, or shrinks on a safe decoder reset. Fragment
assembly retains its reservation while a fragmented message is incomplete.
After checking the actual assembled length, intermediate growth caps geometric
spare capacity at the configured message limit. When a final fragment requires
growth, the decoder uses the now-known complete length, even if the configured
ceiling is larger. Existing sufficient capacity is retained. A 3 MiB prefix
followed by a final 1 MiB therefore grows assembly to 4 MiB rather than 6 MiB,
including under the default 16 MiB message limit. Binary and text assembly use
the same rule, including limits below the usual minimum growth size. Exceeding
the actual message limit still fails before growth admission.
After a message completes, its high water can remain until the configured large
input reclamation policy runs or the session ends. This conservative headroom
can refuse growth while RSS remains lower than the limit; disabling reclamation
also retains those allowances longer. `0` disables this admission policy and
uses the decoder's original allocation-growth behavior.

The staging allowance is not an RSS cap: published `Bytes` bodies retain their
shared owners outside it, consumers have separate output/variable limits, and
allocator overhead, protocol metadata, stacks, TLS/proxy/kernel buffers and
allocator-retained pages are excluded. It controls the native relay decoder;
the optional migration bridge has its own bounded connections/messages and
separate process memory. Use connection and message limits alongside these
reservations and measure actual RSS on the intended deployment.

Native workers process at most 256 WebSocket frames per ready connection turn;
the legacy bridge uses the same frame bound per stream poll. Long sequences of
empty or small continuations yield and explicitly reschedule the reader,
preserving incomplete messages and their existing storage.
This lets other sessions, management requests and cancellation/deadline branches
make progress even when fragment input is already buffered. Complete messages
usually return before reaching the quota. It does not reject otherwise valid
fragmentation or change the message/rate limits. The quota bounds frame count,
not elapsed time or total connection CPU; one large frame still performs its
normal validation, unmasking and assembly work. See the measured scheduling
results in `performance.md`.

Routed output borrows immutable receive storage. A small slice can retain its
backing slab, so routing receives the current allocation's capacity from the
decoder. Shared messages smaller than one eighth of a slab larger than the
configured base chunk have their encoded value compacted before fanout. This
applies to small coalesced views of a large current allocation. Later small
messages on a fresh base slab remain shared; a previous large message alone does
not cause copying. Changing CBOR sender-ID width never shifts the body.
Queue gauges count the emitted CBOR envelope's logical bytes, including its
separate routing prefix, rather than the backing allocation's capacity. Measure
RSS alongside these gauges; they are not a physical-memory accounting interface.

Incomplete input and fragment-assembly allowances can remain resident on open
connections after output drains. The historical per-connection buffer figures
in `performance.md` describe the retired async core; the readiness replacement
shares base storage by worker.

By default, `WAR_READ_BUFFER_IDLE_SECS=5` releases unused decoder storage after
five seconds without a completed binary input larger than the greater of 64 KiB
and the configured initial read buffer. A new large input postpones the deadline;
small input, Ping/Pong and outgoing application traffic leave it alone. Sessions
stay connected and routing continues. Set the interval to `0` to disable quiet
private decoder/fragment resets, or to `1..=86400` for another interval. Completed
oversized worker slabs retire regardless of this timer; recurring large input
can allocate receive storage again. Ordinary base-size input reuses the worker
slab without copying its application body.

Reclamation replaces only the decoder's receive buffer with its initial read
capacity. Published payloads keep their existing shared owners; incomplete
message assembly and protocol/control state remain intact. An incomplete next
WebSocket header may be copied, at most 13 bytes. A complete buffered header or
partially received frame defers the operation for another interval so application
bytes are not moved or lost. Fragment assembly retains its own storage and
allowance while incomplete. Routing uses current storage capacity independently
of this timer.
This needs a narrow local codec hook; its source provenance and complete patch
are in [vendor/README.md](../vendor/README.md).

This reclamation policy releases unused input storage and its staging headroom.
Admission controls growth separately. Incomplete frames and fragment assembly
still need their storage, recipients can still own published bodies, and an
allocator can retain freed pages. Monitor process RSS alongside staging and
logical output reservations.

For applications that should retire inactive sessions, set
`WAR_IDLE_TIMEOUT_SECS=300` (or another value in `1..=86400`). The default `0`
preserves indefinite application-idle sessions. A completed binary input or a
successfully written relay message refreshes activity, including commands and
replies. Receive-only clients remain active while output arrives. Ping/Pong
traffic does not refresh application activity, and an unfinished WebSocket data
message does not qualify as completed input. Queued/in-flight output is active
work governed by the existing queue and write limits.

An application-idle decision sends `1001` / `Application Idle` and increments
`war_idle_timeouts_total`. Routing membership is removed before the handshake;
ordinary channel variables release when the last member leaves. Global-channel
variables retain their existing persistent lifetime. TCP capacity and the
decoder's own storage references release when the transport ends. Published
shared bodies remain alive until their recipients finish or discard them.
An unresponsive peer remains subject to the close deadline.
This policy retires inactive sessions; input-size/admission limits and
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
until the transport ends. The socket owns that reservation, including when an
in-flight private route retains its target beyond decoder retirement.
Policy cancellation interrupts an in-flight socket
write immediately. A partial or failed frame is never followed by a second
blocking flush; that transport is dropped, so an unread client may observe a
reset/abnormal close instead of the intended WebSocket close code. Responsive
clients receive and acknowledge the normal close frame. A received Close is
acknowledged by flushing the exact response already queued by the WebSocket library.
The worker attempts that flush immediately. If TCP blocks it, the connection
waits for writable readiness within the original close deadline; it does not
read the closed decoder again and discard the buffered response. This also
applies when the peer acknowledges a server-initiated Close before output finishes.
Each retained frame records its write offset. A zero offset proves that frame
has performed no I/O, allowing a normal close handshake after cancelling the
queue. A nonzero offset requires transport retirement. The body remains shared.
An untouched stream may remain until the configured close deadline while waiting
for acknowledgement; its routing registration and queued bytes are already released.
Valid peer Close frames bypass input rate accounting before and after channel
join, so an exhausted quota cannot prevent cleanup. Data, joins, Ping, and Pong
retain their configured rate limits.

Terminal framing/decoder failures send a best-effort policy Close and retire
after flushing it. Their denied or invalid body cannot safely be reused to parse
a Close acknowledgement. Complete CBOR policy failures retain the ordinary
acknowledgement exchange.

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
`war_handshake_errors_total`, `war_origin_rejected_total`, `war_join_rejected_total`, `war_protocol_errors_total`, and
`war_rate_limited_total`. Metric labels deliberately exclude user/channel IDs.
Debug logging reports connection failures without message bodies or credentials.

`war_output_queue_capacity_bytes` reports allocated native output queue slots,
including unused retained capacity. `war_output_queue_peak_capacity_bytes` is
the process-wide high-water mark of that aggregate. These count requested slot
storage, not message bodies, allocator bookkeeping or kernel buffers. They are
separate from `war_queued_bytes`, which charges encoded application bytes until
completion or cancellation. A drained queue may retain up to 32 slots for reuse;
larger empty queues release their storage. Terminal output releases even small
queues. Freed storage can remain in allocator arenas, so a lower capacity gauge
does not imply an immediate RSS decrease. Metrics have no user/channel labels.

`war_join_rejected_total` counts native rejection decisions after HTTP upgrade
and before channel registration: invalid joins, unsupported transport, decoder
protocol/size/staging errors, channel admission, rate policy and join timeout.
It increments once before attempting the policy Close. HTTP admission and failed
handshakes retain their existing counters; peer-Close paths and service shutdown
are excluded. This includes peer Close codes normalized by the WebSocket library
to a protocol-error reply: those still follow the peer-Close path. Abrupt
pre-join transport loss is a drop, rather than a rejection decision.
The counter describes a lifecycle stage and can overlap reason counters such
as `war_rate_limited_total` or staging-admission denials. Joined-session protocol
and transport counters retain their existing scope. No user/channel labels or
new per-message accounting are added.

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

`war_read_buffer_reclaims_total` counts completed decoder resets, not bytes
returned to the operating system. `war_read_buffer_reclaim_header_bytes_total`
counts only the incomplete wire-header bytes copied during those resets;
application-value compaction remains in `war_routing_copied_bytes_total`.
`war_read_buffer_reclaims_deferred_total` counts scheduled attempts postponed
because a frame or buffered input still needs processing.

`war_read_buffer_reserved_bytes`, `war_read_buffer_peak_reserved_bytes` and
`war_read_buffer_limit_bytes` expose the current, peak and configured staging
allowance. Reserved/peak include two-times capacity headroom and are not physical
allocation measurements. `war_read_buffer_reservations_rejected_total` counts
refused handshake admissions and frame/fragment growth attempts. A high ratio of
reserved to limit plus rising refusals identifies input staging pressure; it is
separate from slow-consumer and output-budget pressure. With admission disabled,
these four metrics are zero.

## Scaling boundary

Native decoders live in a worker-owned connection arena. Ready queues carry
monotonic socket tokens, so reusing an arena slot cannot redirect an old event to
a new connection. Each connection has at most one indexed timer; repeated input
does not accumulate stale deadline entries. Once a worker becomes empty, an
arena larger than 256 slots and its timer/token tables are released. Small arenas
remain available for churn. An arena can retain its high water while the worker
has live connections; this is separate from the sparse registry trimming below.

I/O workers own dedicated OS threads and are independent of an embedder's Tokio
blocking-pool capacity. On Linux each worker has a separate `SO_REUSEPORT` listen
socket on the supplied address, preserving its IPv6-only setting. New flows are
distributed by the kernel; they are not balanced by channel or current load.
Other platforms use cloned listeners. Management and process signals use Tokio.
Dropping the service or losing a worker suppresses shutdown notifications before
retiring membership, including when the embedding future is cancelled.

Full HTTP channel listings copy bounded names and numeric metadata into an owned
snapshot under the registry read lock, then release the lock before base64
encoding, sorting and JSON serialization. Each encoded name occupies at most
24 inline bytes; the response avoids allocating separate strings for every row.
The JSON buffer starts with an estimate based on row count and can grow for
escaped names or large counters. Its storage transfers to the HTTP body without
a second body copy. The public Rust `Relay::channels()` API still returns owned
strings. Full listings remain complete arrays sorted by `name_base64`; their
output size and sorting work grow with the number of channels. Bounded pages
select only the requested number of small rows while scanning under the read
lock, then order and serialize them after unlocking. See
[bounded browsing](#bounded-channel-browsing) and the
[listing measurements](performance.md#http-channel-listing-allocation-2026-10-16)
before choosing an inspection frequency for a large population.

Native ownership, membership and channel-name tables release excess capacity
after large departures. Occupied fixed-ID takeovers also release sparse ownership
slots.
This keeps a later small audience from scanning the previous population's empty
buckets during global broadcast, room broadcast, or global-listener inspection
on private delivery. Retiring many ordinary rooms also releases spare channel-name
slots used by authenticated channel queries and management inspection.
The check runs under the existing registry write lock when an entry retires;
normal messages add no allocation, trimming check or body
movement. Small tables retain their storage, and larger tables keep regrowth
headroom. A subsequent population wave can allocate metadata again. The
[ID-index measurements](performance.md#id-registry-capacity-after-population-waves-2026-10-16),
[membership measurements](performance.md#membership-index-capacity-after-room-departures-2026-10-16)
and [channel-name measurements](performance.md#channel-name-capacity-after-room-waves-2026-10-16)
record methods, costs and limits.

Rehashing moves registry metadata: ID keys, `Arc` handles and channel descriptors.
Peers, their queues, channel-name/variable bytes and published application bodies
keep their existing owners and addresses. Ordinary variables release when the
last member leaves; global variables retain their persistent lifetime. Socket
state and allocator-retained pages have separate lifetimes, so this is not a
process RSS cap. Displaced sessions continue counting toward connection/session
limits until their normal cleanup.

Workers share one process-local registry; private routing and variables do not
cross process boundaries. Multiple independent replicas need an explicit ownership
and routing scheme before applications can treat them as one relay. Measure
connection churn, active-frame sizes, fanout, queue pressure, p99 latency, CPU,
and memory on the intended TLS/network path before increasing capacity. The
initial measurements cover local single-host sockets, not a deployed cluster.

Full channel inspection captures a coherent view of bounded channel names and numeric
metadata under the registry read lock. UTF-8/base64 formatting, sorting, CBOR
encoding and reply enqueueing run after that lock releases. CBOR queries capture
only names and member counts; HTTP inspection also captures variable usage.
Concurrent joins/departures can make a returned snapshot older than the current
registry by the time the caller receives it. There is no inspection cache.
The snapshot owns small metadata, so it keeps no peer, variable value or routed
application body alive. Each large inspection still allocates snapshot/output
storage and uses CPU proportional to the list, with sorting costs in addition.

### Bounded channel browsing

`GET /v1/channels?limit=100` returns at most 100 rows, with the same JSON array,
fields and ascending `name_base64` order as the full listing. The limit must be
in 1–1,000. Without these query parameters, `/v1/channels` returns the full array.

To continue, set `after` to the last returned row's `name_base64`. The cursor is
exclusive: the next page contains names strictly greater in that base64 text
order. Use standard base64 with its original padding, and URL-encode it because
`+`, `/` and `=` can occur. With the management token configured, for example:

```sh
curl --fail --get \
  -H "Authorization: Bearer ${WAR_ADMIN_TOKEN}" \
  --data-urlencode 'limit=100' \
  http://127.0.0.1:1339/v1/channels

# Use the actual last row's name_base64 here; /wA= illustrates a binary name.
curl --fail --get \
  -H "Authorization: Bearer ${WAR_ADMIN_TOKEN}" \
  --data-urlencode 'limit=100' \
  --data-urlencode 'after=/wA=' \
  http://127.0.0.1:1339/v1/channels
```

Stop when a page has fewer rows than the requested limit. If the final page has
exactly that many rows, the following request returns an empty array. The empty
channel has `name_base64: ""`; its cursor is valid and must be sent explicitly as
`after=`. An absent `after` starts from the beginning. Do not treat an empty
cursor string as an absent cursor.

Each page captures one coherent view. A sequence of pages does not retain a
historical registry snapshot: member/variable counts can change, names can
retire, and new names at or before the last cursor require a fresh traversal to
observe them. A retired cursor remains usable. The server stores no cursor
state and keeps no session or variable body alive for subsequent pages.

Invalid limits, malformed/noncanonical base64, cursors longer than a 16-byte
channel name, duplicate recognized parameters, or `after` without `limit` return
HTTP 400. Missing or incorrect authorization returns HTTP 401 before query
validation.

Page metadata storage and output depend on the requested limit. Selecting a page
still scans the channel registry under a read lock; a full traversal therefore
repeats that scan. Use the full endpoint when collecting every channel at once,
and bounded pages when browsing a limited part of a large population. The
[paging measurements](performance.md#bounded-http-channel-pages-2026-10-16)
record both costs. CBOR channel-query command 2 keeps its existing full-list
behavior and raw-byte ordering.
