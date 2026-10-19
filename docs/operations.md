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

## 100,000 native connections

The [100,000-user environment example](../deploy/war-relay-100k.env.example)
and [systemd override](../deploy/war-relay-100k.conf.example) provide a separate
native CBOR operating profile. Copy the environment example to
`/etc/war-relay.env` and the override to
`/etc/systemd/system/war-relay.service.d/100k.conf`, then reload systemd before
starting the service. Set the secrets, allowed browser origins and TLS edge for
your deployment. These files are examples; installing them is an administrator's
action. The ordinary 5,000-user environment example remains available.

The base unit's `LimitNOFILE=65536` cannot accommodate 100,000 relay sockets.
The override raises it to 262,144 and supplies 6 GiB/8 GiB memory pressure/hard
guardrails, including the service's charged kernel memory. One
native user needs one relay descriptor plus shared listener, reactor and
management descriptors. Size the TLS proxy separately: each proxied user also
needs a public and backend proxy socket. Check the running processes' actual
limits in `/proc/<pid>/limits`, descriptor counts in `/proc/<pid>/fd`, and the
proxy's own connection limits. A shell's `ulimit` does not establish the limits
of a systemd service.

The profile caps incoming messages at 64 KiB, per-recipient pending data at
256 KiB/64 messages, process pending data at 256 MiB and shared receive staging
at 256 MiB. Its TCP ceiling is 110,000, leaving headroom for handshakes and
closing/reconnecting transports around 100,000 joined users. It allows 10,000 channels and 1,024
members per channel. Authenticated global broadcast remains available; ordinary
global broadcast is disabled. These are admission ceilings, rather than
preallocated buffers. Large-room broadcasts and departure notifications still
cost work per recipient. Select limits to match the application's fanout and
latency requirements; connection count alone does not specify traffic capacity.

Idle native sockets retain no dedicated receive allocation. Workers reuse
receive scratch; incomplete frames retain separately admitted storage. Ordinary
routing shares the validated application body and uses vectored writes for
the WebSocket header, CBOR sender prefix and body. Kernel socket storage, TLS
proxy buffers and allocator overhead add to relay RSS. Kernel TCP memory and
cgroup memory accounting must be observed alongside process memory.

Native `--tcp-send-buffer-bytes` / `WAR_TCP_SEND_BUFFER_BYTES` and
`--tcp-receive-buffer-bytes` / `WAR_TCP_RECEIVE_BUFFER_BYTES` request kernel
buffers on each accepted socket, before HTTP upgrade. `0` preserves OS defaults;
positive requests must be between 4 KiB and 16 MiB. A rejected socket setting
uses the existing setup-error/admission-retry path. The 100,000-user example
requests 16 KiB send and 64 KiB receive buffers. A 16 KiB receive request added
hundreds of milliseconds to near-64 KiB packets on loopback; the larger request
removes that observed stall. Linux normally reports twice the requested
value for bookkeeping and may clamp requests to kernel limits; see
[socket(7)](https://man7.org/linux/man-pages/man7/socket.7.html).
These settings constrain a different layer from the application queues. Smaller
windows can constrain throughput on high-latency networks; tune and verify them
against the target network's bandwidth-delay product. Kernel buffer requests
are not a complete accounting of TCP memory, allocator state or TLS edge costs.

To reproduce the population acceptance on a Linux test host with enough file
descriptors and available memory:

```sh
cargo build --locked --release --bins --example capacity --example stress
python3 scripts/capacity.py --users 100000 --seconds 65 \
  --ping-interval-secs 15 --churn-workers 16 \
  --connection-headroom 10000 \
  --tcp-send-buffer-bytes 16384 --tcp-receive-buffer-bytes 65536 \
  --output artifacts/capacity-100k.json
```

This creates a fresh isolated loopback relay. It uses four physical CPU cores
for the server and eight different cores for the client by default; override
`--server-cpus` and `--client-cpus` for the host, or pass empty strings to leave
affinity unrestricted. A pool of source addresses avoids the ephemeral-port
ceiling of a single client address. The driver changes no host limits or
sysctls. Every stable socket must first deliver a full checked ring message.
The timed workload sends one 1 KiB private message per user per second;
`--messages-per-second`, `--active-percent` and `--payload-bytes` vary that load.
Additional churn rooms verify complete lifecycles while the stable population
remains connected. Inactive clients answer heartbeats. The controller checks
management responses, exact counters, actual established TCP sockets, all
departure messages, Close acknowledgements, EOF and final resource release.
After the timed workload, a separate checked ring proves every stable user's
application routing still works, including the inactive population. This sweep
is outside the timing interval.
`--waves 2` repeats complete population/traffic/retirement on the same process.
An optional `--pressure-client target/release/examples/stress` adds one verifying
fanout reader and eight unread recipients during a single wave of at least
25 seconds. Its exact input, shared-byte and lifecycle totals are checked;
intentionally cancelled output has explicit bounds and full frame validation
by the pressure client. Expected slow-consumer and missing-close-ack counters
are separated from unexpected errors. All stable users must still complete
their routing and normal Close/EOF checks.

The workload is plaintext loopback evidence. A deployment's TLS edge, network,
message sizes, fanout, admission bursts and required latency need corresponding
acceptance on that deployment. See the [capacity measurements](performance.md#100000-connected-users-2026-10-16)
for the tested artifact and observed limits.

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

## Pending TCP connections

Both binaries accept `--listen-backlog` / `WAR_LISTEN_BACKLOG`, default `1024`,
with requested values in `1..=65535`. This controls completed TCP handshakes
waiting for userspace `accept`; the OS may cap the request. It applies to every
native data listener and the bridge CLI's public listener. The separate native
management listener retains its HTTP runtime's backlog.

Native workers process at most 16 sockets per accept turn before returning to
queued work. This scheduling budget is separate from the kernel backlog and
established-connection limits. The [fairness measurements](performance.md#accept-turn-fairness-2026-10-16)
record the established-traffic benefit and refusal-throughput tradeoff.

The native HTTP upgrade has an absolute five-second deadline by default;
partial headers do not extend it. Its read allowance is held until upgrade,
failure or timeout, and unfinished EOF/timeouts increment
`war_handshake_errors_total`. The reserved allowance is conservative staging
headroom, rather than measured heap usage. The
[pending-header checks](performance.md#inline-http-upgrade-state-2026-10-16)
verify timeout release and subsequent admission while established CBOR traffic
and management metric requests remain usable.

```sh
./target/release/war-relay --workers 4 --listen-backlog 2048
./target/release/war-legacy-bridge --listen-backlog 2048
ss -ltn 'sport = :1338'
```

On Linux, LISTEN `Recv-Q` shows currently queued connections and `Send-Q` shows
the effective accept backlog. Larger queues can absorb bursts and retain more
kernel socket memory. They do not increase established-session quotas or
application buffer budgets. Kernel listen drops occur before userspace accept
and are not relay `war_rejected_total` or `war_accept_errors_total` events.
Inspect kernel TCP listen-overflow/drop and retransmission counters when clients
time out while relay error counters remain quiet.

Native `serve` applies `Config::listen_backlog` to a supplied pre-bound listener
and all additional worker listeners. Embedders that previously chose their own
backlog should set that value in `Config` too. Bridge embedding functions retain
the supplied listener's backlog; the bridge CLI applies its requested setting.
The [admission measurements](performance.md#tcp-admission-continuations-and-backlogs-2026-10-16)
record effective queues and the workload limits.

## Direct-peer connection admission

Both binaries accept `--max-connections-per-ip` / `WAR_MAX_CONNECTIONS_PER_IP`.
The default `0` disables this policy. A positive value caps admitted concurrent
TCP connections from each direct peer IP, shared across native I/O workers.
For example:

```sh
./target/release/war-relay --max-connections-per-ip 32
./target/release/war-legacy-bridge --max-connections-per-ip 32
```

The limit includes unfinished HTTP upgrades, unjoined WebSockets, joined
sessions and closing transports. The native global cap is checked first; a
per-IP refusal closes TCP before HTTP/WebSocket upgrade and increments both
`war_ip_rejected_total` and `war_rejected_total`. Denied connections receive no
WebSocket Close frame. Existing socket owners retain their address lease through
queued output, retirement and captured routing targets. EOF, handshake/join
failure, timeout, normal close and service cancellation release the lease.

Addresses come from the accepted TCP connection. IPv4-mapped IPv6 addresses use
the same quota as their IPv4 form; other IPv6 addresses each have their own
quota. HTTP `Forwarded`, `X-Forwarded-For` and similar headers do not change this
identity. NAT users share a quota. A TLS reverse proxy's backend connections
share its address too: enforce client-address quotas at that edge, or size/
disable this backend limit for the proxy's aggregate. The legacy bridge's
upstream connections likewise share its source address at the native relay.
Configure public ingresses independently; enabling only the native limit
limits the bridge's aggregate upstream connections rather than each downstream
client's address.

The index allocates one shared entry per active canonical address. Connection
leases reuse those entries, and their final owner removes them; sparse index
capacity trims after large address populations retire. Disabled policy allocates
no IP index. Admission does no per-message parsing or accounting. This bounds
concurrent transports; applications needing per-user or tenant quotas still
need an identity-aware edge or separate relay instances.

Existing `legacy_bridge::serve` and `serve_with_origin_policy` callers retain a
disabled IP policy. Embedders use `serve_with_limits` to set both connection caps
and the origin policy. The bridge has no management listener or IP-denial metric;
`war_ip_rejected_total` describes only native admissions. Accepted configuration
values are `0..=4294967295`, with the shared connection cap still applying.

The [address-churn measurements](performance.md#address-churn-with-complete-lifecycle-validation-2026-10-16)
separately exercise one shared address and a rotating 4,096-address pool with
complete Close/EOF validation. Concurrent quotas do not cap connection rate.

## Accepting under OS resource pressure

An exhausted process descriptor limit can make `accept()` fail before the relay
can apply its connection quotas. Both public ingresses keep existing sessions
running and retry such failures after one second. Native workers temporarily
remove the listener from their readiness poll, continue servicing established
sockets and deadlines, then register it again and explicitly accept queued
flows. The bridge keeps its connection tasks and shutdown responsive during the
wait. A retry does not create a per-connection timer or change payload storage.

`war_accept_errors_total` counts native accept failures other than normal
`WouldBlock` and `Interrupted` results. It is separate from `war_rejected_total`:
no connection was accepted when the syscall failed. Resource failures log a
warning at each retry; aborted/refused/reset incoming flows retry immediately
within the existing bounded work schedule. The bridge uses the same retry policy
and logs its failures, but has no management metric endpoint.

An accepted native socket can also fail during `TCP_NODELAY` configuration or
readiness registration. The worker closes that socket and suspends its listener
for one second while existing sessions continue. Other native workers remain
available. `war_setup_errors_total` counts these failures; each also increments
`war_rejected_total`. They do not increment `war_accept_errors_total` or HTTP
handshake errors. Both the global connection slot and an enabled IP permit stay
owned until the physical descriptor closes, including setup refusal paths.
Failure to suspend or re-register the listener itself remains a worker error.

Queued TCP connects can complete before their HTTP upgrades are accepted.
When capacity returns, those queued flows can proceed without another connect
event. Shutdown cancels retry waits; responsive native peers still receive
`1001`. New management connections also need descriptors during exhaustion;
a retained management HTTP connection remains usable in the fault check.
Keep headroom below the OS limit for listeners, pollers, signals and management,
and for the bridge's downstream and upstream sockets.

The Linux regression probe limits only disposable children to 64 descriptors:

```sh
cargo build --locked --release --bins
python3 scripts/accept-pressure.py --output artifacts/accept-pressure.json
```

It covers one/four native workers and the bridge, verifies existing private
replies at the limit, recovers an already queued flow, waits for pressure sockets
to retire, and shuts down during a second exhaustion wave. The same probe runs
against debug binaries in Linux CI. The test does not change host limits or
simulate system-wide descriptor/memory exhaustion. See the
[recorded behavior and costs](performance.md#accept-recovery-under-descriptor-pressure-2026-10-16).

The separate setup probe compiles a small Linux fault library with `cc` and
loads it only into disposable child servers:

```sh
python3 scripts/setup-pressure.py --output artifacts/setup-pressure.json
```

It injects one `TCP_NODELAY`/`ENOBUFS` or socket-registration/`ENOMEM` failure at
each armed gate, with one/four workers. A second gate holds the real descriptor
open while management checks admission ownership. Each case verifies two
recoveries and shutdown during a third retry. Recovered clients complete exact
Close/EOF; shutdown checks the existing flush-Close-and-retire behavior. Linux
CI runs this probe too. This tests recovery from the syscall results, without
exhausting host memory or kernel watch capacity. See the
[setup-failure evidence](stability.md#accepted-socket-setup-failures-2026-10-16).

## Important defaults and budgets

`war-relay --help` is the source of truth for CLI options and matching `WAR_*`
variables. All bounds are per process.

| Budget | Default | Counts |
| --- | ---: | --- |
| Connections | 10,000 | TCP handshakes, unjoined sockets, joined sessions, and closing transports |
| Pending TCP accept backlog | 1,024 per public data listener | Kernel queue outside userspace admission counts; OS may cap the requested value |
| Native kernel TCP buffers | OS defaults (`0`) | `--tcp-send-buffer-bytes` / `--tcp-receive-buffer-bytes`; independent of application queue/staging budgets; OS may clamp or adjust requests |
| Connections per direct peer IP | Disabled (`0`) | Optional native/bridge TCP admission; shared NAT/proxy addresses share a quota |
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

Native HTTP-upgrade and channel-join deadlines are absolute within their phases.
Dripped header bytes and WebSocket Ping/Pong traffic do not extend them. A join
timeout starts a 1008 policy Close; the TCP admission lease remains held until
the transport retires through acknowledgement or its close deadline. See the
[short stalled-ingress checks](benchmarks/2026-10-18-stalled-ingress/README.md)
for silent/dripping HTTP peers and silent/Ping-active upgraded peers beside
verified healthy traffic. The join fixture deliberately uses two seconds rather
than the ten-second production default to keep the regression short.

`join-pressure.py --withhold-close-ack` additionally uses a one-second fixture
close budget. It receives the policy Close without replying, verifies that an
extra TCP connection is refused while closing peers still hold the configured
cap, then checks deadline EOF and a successful recovery join. The expected
`war_close_failures_total` counts those deliberately missing acknowledgements;
it is distinct from a write failure or failed healthy delivery. See the
[unacknowledged-close check](benchmarks/2026-10-18-unacknowledged-close/README.md).

Queue limits concern logical output reservations, including charged active fanout
membership snapshots in the global allowance, not the complete process RSS. WebSocket
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

Native multicast submits shared frames to each socket's owning worker, which
writes up to 16 frames / 32 iovecs per syscall within the existing 256 KiB
per-flush write quantum. This is a scheduling batch, not a message-size limit or
a wait-to-fill threshold. Worker notifications coalesce while work is pending.
Ordinary private traffic without observers keeps its immediate vectored-send
path and its original frame representation. Private routes with observers use
the multicast method. Payload bytes stay shared; small headers are built on the
writer's stack. Queue storage allocates on first use, reuses warm slots and
retains the existing capacity reclamation and admission policies. Reservations
survive partial writes until the frame completes or is cancelled. The
[measured implementation record](benchmarks/2026-10-17-rust-owner-fanout/README.md)
includes short writes, slow-reader isolation, latency and full-population checks.

Ordinary channel broadcasts without global channel observers capture a shared
view of recipient handles under the registry lock, then release that lock before
submitting output. Stable membership reuses the view. A join or departure updates
it in place when no routing owner has captured it; otherwise it invalidates the
cache and leaves existing captures intact. The cache owns peer handles, never application values. In-flight
views keep their captured owners valid across membership changes, and frames
retain their own body storage and byte reservations.

Reusable views can retain spare handle capacity after growth. A retained room
releases sparse capacity when more than 256 slots remain and live membership is
at most one eighth of capacity, keeping twice the live size after shrinking.
Empty ordinary rooms drop the view completely. Spare slots do not own departed
peers, sockets or bodies. The [membership measurements](performance.md#recipient-views-during-membership-churn-2026-10-16)
record allocation savings, retention costs and the timing limits.

Those broadcasts also charge a native read turn for recipient slots visited,
with a quota of 256. A large broadcast still completes against one coherent
view before yielding; the next buffered message waits for another turn. This
keeps payloads shared and sender FIFO intact while allowing other connections,
admission and timers to run. It is a work quota, not a time deadline: a single
broadcast may visit more than 256 peers. Global broadcasts and routes with
global observers also charge recipient visits, preserving complete-message
submission and per-source FIFO before yielding.
See the [large-room measurements](performance.md#large-room-fanout-fairness-2026-10-16)
for latency, throughput and metadata costs.

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
Readiness becomes `503`; liveness stays `200`. POST `/v1/resume` returns `204`
and restores admission while the relay is manually drained. Alternatively, wait
for sessions to leave and send SIGTERM for bounded closure.

Shutdown is terminal. After SIGINT/SIGTERM, worker failure or cancellation of an
embedded service starts shutdown, a concurrent management request cannot reopen
admission. An authorized POST `/v1/resume` returns `409 Conflict`; POST `/v1/drain`
remains an idempotent `204`. Authentication still precedes either operation, so
requests without a valid token return `401`. While the management listener
remains available, `/readyz` stays `503` and `war_draining` stays `1`.

Rust embedding API: replace `relay.draining.load(...)` with
`relay.is_draining()`, and `relay.draining.store(value, ...)` with
`relay.set_draining(value)`. Check its boolean result: `false` means a resume
was refused because shutdown has started. `relay.is_stopping()` distinguishes
terminal shutdown from a reversible manual drain. The lifecycle uses one atomic
value; stopping cannot be overwritten by a concurrent drain/resume transition.
Create a new `Relay` when restarting an embedded service.

The optional legacy bridge has its own shutdown policy. SIGINT/SIGTERM cancels
its connection tasks and releases both TCP legs, including pending HTTP upgrades.
Active clients can see transport termination without a WebSocket Close frame.
Their upstream native sessions contribute to `war_transport_disconnects_total`;
that expected cancellation count alone does not mean healthy forwarding failed.
A direct native session can continue while the bridge stops. The
[short bridge shutdown check](benchmarks/2026-10-18-bridge-shutdown/README.md)
verifies this behavior and final resource retirement. Wait for an idle bridge
before a local binary replacement when existing legacy sessions must be preserved;
the guarded local update helpers enforce that precondition.

An application host can restart and reclaim its fixed ID without restarting the
relay or unrelated sessions. Restarting the relay itself ends its sessions and
loses in-memory variables; clients must reconnect and rebuild application state.

Monitor `war_connections`, `war_sessions`, `war_channels`, `war_queued_bytes`,
`war_variable_bytes`, `war_slow_consumers_total`, `war_global_pressure_total`,
`war_handshake_errors_total`, `war_origin_rejected_total`, `war_join_rejected_total`, `war_protocol_errors_total`, and
`war_rate_limited_total`. Metric labels deliberately exclude user/channel IDs.
Debug logging reports connection failures without message bodies or credentials.

Native cumulative input counters (`war_incoming_messages_total`,
`war_incoming_bytes_total`, `war_routing_reused_bytes_total` and
`war_routing_copied_bytes_total`) publish at each worker's existing ready-batch
boundary, before it polls again. An active worker can temporarily have unpublished
input statistics; totals are exact once its batch finishes. Worker exit also
publishes remaining totals. Admission, retained-byte gauges, pressure decisions
and outstanding-byte accounting remain immediate. Completion counters publish
at vectored-write boundaries. No quota depends on deferred
statistics. See the [input-counter measurements](performance.md#worker-local-input-statistics-2026-10-17).

`war_output_queue_capacity_bytes` reports allocated native output queue slots,
including unused retained capacity. `war_output_queue_peak_capacity_bytes` is
the process-wide high-water mark of that aggregate. These count requested slot
storage, not message bodies, allocator bookkeeping or kernel buffers. They are
separate from `war_queued_bytes`, the exact global output-admission counter. It
charges in-flight encoded application bytes until completion or cancellation
and active fanout membership snapshots until their continuation releases them.
Shared bodies may be charged once per recipient, and concurrent publishers can
conservatively charge the same membership view separately. This is logical
admission accounting, not unique resident payload memory or a process RSS cap.
A drained queue may retain up to 32 slots for reuse;
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
does not accumulate stale deadline entries. On retirement, an arena with more
than 256 slots compacts when at most one eighth of its capacity remains live.
The worker updates token-to-slot mappings and preserves each indexed deadline.
Ready events and cross-worker notifications retain their socket tokens. This
moves decoder metadata; socket owners and shared application bodies keep their
allocations.

Compaction also trims timer storage. An empty worker releases an arena larger
than 256 slots, or an arena that previously needed compaction, along with its
timer/token tables. Small arenas that never needed compaction remain available
for ordinary reconnect traffic. Repeated large population waves can allocate
metadata again; compaction runs on the retiring connection's worker. The
[population recovery measurements](performance.md#compact-sparse-connection-arenas-2026-10-16)
separate its memory benefit from traffic and retirement costs. Allocators can
retain freed pages, so arena reclamation does not set a process RSS ceiling.

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

## Measuring the native message path

Size a relay for delivered messages and bytes per second, room fanout, churn and
unread peers alongside its connected population. Use separate client/server
physical cores when measuring CPU scaling, and record the actual workload and
TCP buffer requests. A million offered messages/s is not a million completed
messages/s. Inspect scheduling backlog together with RTT quantiles and CPU.

The capacity harness supports a separate kernel-inclusive diagnostic:

```sh
python3 scripts/capacity.py \
  --users 100000 --seconds 5 --messages-per-second 10 \
  --tcp-send-buffer-bytes 16384 --tcp-receive-buffer-bytes 65536 \
  --profile --profile-kernel --output artifacts/kernel-diagnostic.json
perf report --stdio --no-children -g none \
  --sort dso,symbol -i artifacts/kernel-diagnostic.perf.data
```

This starts a disposable local server and requires built native/client binaries,
sufficient descriptor limits and perf permissions covering kernel events.
Without `--profile-kernel`, `--profile` captures user cycles only. Run ordinary
comparisons separately from diagnostics; keep message checks and cleanup enabled.
For a real NIC, inspect receive/transmit queue placement and CPU topology before
changing interrupt or flow steering; the [kernel scaling guide](https://docs.kernel.org/networking/scaling.html)
describes RSS, RPS, RFS and XPS. The [C10M-guided measurements](performance.md#c10m-guidance-and-the-socket-send-path-2026-10-17)
record the current socket-call improvement and the prototypes removed after
measurement. They cover loopback without a TLS edge or remote NIC.

## Evaluating AOCL-LibMem

The Linux benchmark tools accept an explicit **server-only** shared-library
preload. This allows the same frozen relay and verifying client to compare
glibc with [AMD AOCL-LibMem](https://github.com/amd/aocl-libmem), preserving the
relay's shared CBOR bodies and vectored output. The library replaces eligible
out-of-line libc calls; compiler-inlined operations, Rust's own SIMD routines
and kernel TCP copies remain separate. A successful preload does not establish
a performance improvement. See the [measured AOCL study](performance.md#aocl-libmem-evaluation-2026-10-17)
before selecting it for an operating workload.

Build a pinned library inside the ignored artifact directory:

```sh
git clone --depth 1 --branch 5.3.2 https://github.com/amd/aocl-libmem.git artifacts/reference/aocl-libmem-5.3.2
cmake -S artifacts/reference/aocl-libmem-5.3.2 \
  -B artifacts/aocl-libmem-5.3.2-dispatch -G 'Unix Makefiles' \
  -DCMAKE_C_COMPILER=gcc -DALMEM_DYN_DISPATCH=Y
cmake --build artifacts/aocl-libmem-5.3.2-dispatch --parallel
cmake --install artifacts/aocl-libmem-5.3.2-dispatch
```

Upstream's standalone build installs into its own build directory. These
commands do not install system libraries. The study also builds
`-DALMEM_ARCH=zen5` in a separate directory on its verified Zen 5/AVX-512 host.
That architecture-specific build must only be used on a compatible CPU. The
dispatched build performs CPU selection at runtime. Follow AMD's
[build requirements and CPU compatibility guidance](https://github.com/amd/aocl-libmem/blob/5.3.2/BUILD_RUN.md)
when building on another host. This release's CMake configuration produces
duplicate-output errors with Ninja on the measured host; Makefiles work.

After building the relay and fixed stress client, compare the same executable:

```sh
python3 scripts/compare.py \
  --candidate glibc=target/release/war-relay \
  --candidate aocl=target/release/war-relay \
  --server-preload aocl=artifacts/aocl-libmem-5.3.2-dispatch/lib/libaocl-libmem.so \
  --stress target/release/examples/stress \
  --mode traffic --clients 64 --payload-bytes 1024 --window 16 \
  --seconds 8 --repetitions 3 --output artifacts/aocl-comparison.json
```

`capacity.py`, `stability.py`, `input-corpus.py` and `fragment-waves.py` also
accept `--server-preload PATH`. The client and pressure sidecar retain the
original environment. The runners reject an inherited `LD_PRELOAD`, reject
loader paths containing whitespace or colons, verify the resolved library path
in the server's `/proc/PID/maps` after readiness, and record its SHA-256. A
loader that ignores an invalid library therefore cannot silently produce a
baseline result labelled as AOCL. Mapping verification establishes loading;
the separate loader-binding diagnostic in the study proves which symbols are
actually replaced. Run diagnostics, builds and validation separately from
ordinary timing trials. Avoid setting a machine-wide preload.

The direct upstream C++ comparison accepts repeatable `--cpp-preload
LABEL=LIBRARY` options. Each adds a C++ candidate alongside the glibc C++ and
native Rust controls. For the three pinned libraries from the study:

```sh
python3 scripts/compare-cpp.py \
  --relay artifacts/baselines/c10m-sendmsg/war-relay \
  --client artifacts/baselines/active-pairs-client/inspection \
  --client-source-root artifacts/baselines/active-pairs-client \
  --rebuild-cpp --workers 4 --rooms 8192 --pairs 64 --client-workers 8 \
  --server-cpus 0-3 --client-cpus 4-11 \
  --seconds 6 --repetitions 5 --cases private query \
  --cpp-preload zen5=artifacts/aocl-libmem-5.3.2-zen5/lib/libaocl-libmem.so \
  --cpp-preload avx2=artifacts/aocl-libmem-5.3.2-avx2/lib/libaocl-libmem.so \
  --cpp-preload dispatch=artifacts/aocl-libmem-5.3.2-dispatch/lib/libaocl-libmem.so \
  --output artifacts/cpp-aocl-four-worker.json
```

Build the AVX2 library in its own directory with `-DALMEM_ISA=avx2`; use
`-DALMEM_ARCH=zen5` for the compatible native build. The frozen binaries in
this command belong to the archived checkpoint; when testing a new build,
pass its executable and associated client source snapshot explicitly. C++ is
rebuilt in a temporary directory with only the worker-count expression changed;
the upstream routing, storage and protocol sources remain intact. The runner
clears inherited library preload/search settings, applies the requested
library to the selected C++ server, verifies its mapping and records all loaded
C++ library hashes. It also verifies the client has no AOCL mapping. The glibc
and Rust controls retain their ordinary environments.

See the [C++ AOCL results](performance.md#current-c-versus-rust-with-aocl-2026-10-17)
before selecting a library: loading AOCL succeeds, but these workloads do not
establish a useful improvement. The recorded binding diagnostics run separately
from ordinary timing trials, including a negative test for an ignored library.

To include a frozen native Rust baseline in the same C++ matrix, pass
`--control PATH --control-read-storage worker`. The default `connection` storage
contract remains available for the older asynchronous control. This option
selects the verifier's receive-staging checks; it does not alter either server.

## Rebuilding the Monoio experiment

The [Monoio study](performance.md#monoio-readiness-evaluation-2026-10-17) rejects
the measured readiness adapters as the default. They remain reproducible from
the archived patches without changing the main Cargo project. From this Linux
checkout, use a new output directory:

```sh
python3 scripts/try-monoio.py persistent --output artifacts/monoio-trial
artifacts/monoio-trial/target/release/war-relay --workers 4 \
  --bind 127.0.0.1:1438 --admin-bind 127.0.0.1:1439
```

The script extracts the recorded base commit, applies the checksum-verified
patch, checks all 115 final source hashes, builds with the locked dependencies
and records the compiler and new executable hash. `--prepare-only` skips the
build. `readiness` and `persistent-unfixed` reproduce the historical variants,
including their known retirement gap, for diagnosis. `persistent` includes the
retirement fix. The main native relay continues to use Mio; the experimental
binary selects Monoio's io_uring data workers and retains Tokio management.

This adapter needs two descriptors per admitted user, additional watcher memory
and an available io_uring driver. Large parallel test suites may need a larger
process locked-memory allowance. The study records its original `ENOMEM`
failure and the scoped limit change used for testing. Its results cover
readiness integration; completion-based socket I/O remains a separate experiment.


## Rebuilding owned completion I/O

The [completion study](performance.md#monoio-owned-completion-io-evaluation-2026-10-17)
measures actual owned reads and vectored writes. Rebuild it into a fresh directory:

```sh
python3 scripts/try-monoio.py completion --output artifacts/monoio-completion-trial
artifacts/monoio-completion-trial/target/release/war-relay --workers 4 \
  --bind 127.0.0.1:1438 --admin-bind 127.0.0.1:1439
```

The script verifies the archived patch and all 118 source hashes, builds with
locked dependencies and records the new binary hash. `--prepare-only` validates
sources without building. `completion-initial` reconstructs the historical variant
that fails overload through receive-budget exhaustion; use it only for diagnosis.
The bounded `completion` variant passes the recorded acceptance checks but costs
more CPU and memory and delivers less throughput than selected Mio.

This implementation uses one accepted descriptor per user, worker-local read
and write tasks, a 14-byte idle receive header and frame-sized body reservations.
Its helper crate contains audited unsafe owned-buffer trait implementations;
payload owners and reservation guards remain alive until completion or completed
cancellation. Large parallel suites may require a scoped locked-memory allowance
for their rings. Tokio remains in management/lifecycle. The main project's native
Mio runtime remains selected. These experiments require Linux with available
io_uring and do not imply kernel zero-copy sends or provided-buffer rings.


## Plotting latency over time

Build the instrumented capacity example, then use `scripts/capacity.py` with
`--client target/release/examples/capacity --user-latency-output artifacts/run.csv`.
In addition to per-user CSV, this writes `artifacts/run.windows.json` with exact
one-second intended-send cohorts, pre-send delay and Pings encountered while
reading their probes. This diagnostic requires one wave. Window aggregation and
output happen after timing; keep the resulting client hash distinct from older
frozen benchmark clients.

```sh
python3 scripts/plot-latency-timeline.py \
  --case 'Heartbeat on=artifacts/on.windows.json' \
  --case 'Heartbeat off=artifacts/off.windows.json' \
  --output artifacts/latency-over-time
```

Repeat `--case LABEL=PATH` for each recorded run. The plotter writes PNG, PDF and
a JSON summary. Use matched workloads and both orders; the
[timeline study](performance.md#latency-timeline-and-observed-heartbeat-cohorts-2026-10-17)
records the current four-case on/off/off/on comparison and its limitations.


## Native worker CPU placement

For latency-sensitive Linux service, select a suitable allowed CPU set and pin
one native I/O worker to each CPU:

```sh
taskset -c 0-3 target/release/war-relay --workers 4 --pin-workers
```

`--pin-workers` (or `WAR_PIN_WORKERS=true`) is opt-in. Workers use the first
`--workers` CPUs in the inherited allowed mask, in CPU-ID order. Startup fails
if there are too few allowed CPUs or a worker cannot set its affinity. Choose
physical cores and NUMA placement appropriate to the host; CPU IDs need not
represent separate physical cores. Keep competing workloads off these CPUs.
Management threads retain their inherited mask. This policy adds no work to
the packet path and preserves the existing borrowed payload ownership.

The measured 100,000-user low-latency profile uses four server workers on CPUs
0–3, clients on 8–15, 16 KiB send / 64 KiB receive socket buffers and 15-second
heartbeats. It offers 400,000 1 KiB private messages/s. These are measured host
settings, not a portable capacity guarantee. Maintain load headroom: the overload
profile has much worse latency even when every packet is delivered correctly.

The capacity harness `--pin-workers` requests and independently checks the
native startup option. `--pin-server-workers` and `--pin-client-workers` are
harness-only experimental interventions. `--profile --profile-scheduling` records
scoped CPU-clock/switch sideband for the fresh server and client processes;
profiling changes timing and must run separately from ordinary comparisons.
`scripts/analyze-scheduling.py` analyzes exported switch events. Its off-CPU
intervals include socket waits and are not direct wakeup latency measurements.


### Eight-worker low-latency profile

On the measured host, use eight separate physical server cores when latency
at 100,000 users / 400,000 1 KiB messages/s takes priority over CPU efficiency:

```sh
taskset -c 0-7 target/release/war-relay --workers 8 --pin-workers \
  --max-connections 110000 --ping-interval-secs 15 \
  --tcp-send-buffer-bytes 16384 --tcp-receive-buffer-bytes 65536
```

Compared with four workers on cores 0–3, this lowers the 99th percentile of
user-level p99s from 3.67–4.43 ms to 1.57–1.58 ms in two rotating matched pairs,
while increasing CPU/message about 11% and requiring four more server cores.
This example selects placement, connection headroom and socket buffers; exact
benchmark commands and their other quota/rate settings are in the raw records.
Keep normal service protections appropriate to your deployment. Keep load clients and other CPU-heavy work off
the selected physical cores and their SMT siblings. These CPU IDs describe the
measured machine; inspect topology before applying them elsewhere. The binary
and four-worker development service are unchanged. See the
[measurement and limits](performance.md#eight-worker-latency-operating-profile-2026-10-17).

## Comparing latency headroom without hiding backlog

Use the generic plotter for any per-user capacity CSVs:

```sh
python3 scripts/plot-user-latency-comparison.py \
  --case '600k/s repeat 1=artifacts/latency-headroom-six-1.csv' \
  --case '800k/s repeat 1=artifacts/latency-headroom-eight-1.csv' \
  --output artifacts/headroom-comparison
```

Repeat `--case LABEL=CSV` for each workload/repetition. The output includes PNG,
PDF and JSON with CSV hashes, sample counts and exact nearest-rank user-level
percentiles. Left-hand curves start at actual send; right-hand curves start at
intended send and include generator backlog. Neither curve is a pooled packet
percentile. Optional repeated `--threshold-ms N` reports the fraction of users
whose p99 meets that threshold; it does not establish a packet-loss rate or
latency guarantee. No threshold is imposed by default. Empty data, duplicate IDs,
nonpositive sample counts and invalid latency values fail validation.

A run that eventually verifies every planned packet can still be overloaded.
Compare offered rate with completed rate and inspect intended-send latency and
scheduling lag before selecting operating headroom. The test includes the load
generator, loopback stack and relay; its delay does not isolate a server-only
capacity limit or predict WAN/TLS behavior.

## Joint client/server CPU diagnosis

`--profile --profile-client` records cycle samples for both fresh processes;
`--profile-kernel` includes kernel cycles, and `--profile-frequency 99` can reduce
sampling overhead from the ordinary 499 Hz diagnostic default. Scheduling
sideband already records both processes and excludes `--profile-client`.
Profiles remain separate diagnostic runs, never ordinary performance repeats.
The result records the exact command and `profile_pids` mapping.

Filter and normalize the report for each process explicitly:

```sh
sudo perf report --stdio --no-children --call-graph none --sort symbol \
  --pid CLIENT_PID --percentage relative -i artifacts/joint-600k-profile.perf.data
```

Repeat with the recorded server PID. `--pid` alone retains an absolute combined
percentage denominator; `--percentage relative` is necessary for fractions of
that process's own samples. Per-process user/system CPU deltas are available
from the existing ready/measured event snapshots. Sample fractions and call
chains are diagnostic evidence, not syscall counts or predicted speedups.

## Per-thread counter and CPU-accounting diagnostics

Use `--profile --profile-counters` with the capacity harness to collect `perf stat`
for existing ready-time threads. Add `--profile-client` to include client threads.
This mode includes kernel and user counts and excludes the kernel/scheduling
sample flags. It collects task-clock, cycles, instructions, cache misses,
context switches and CPU migrations, with explicit TID attribution and event
running percentages. No future-thread inheritance is assumed. The collector
records per-CPU `/proc/stat` snapshots around collection and the process CPU
masks, without changing system settings.

```sh
python3 scripts/analyze-perf-counters.py \
  --run artifacts/counter-accounting.json \
  --output artifacts/counter-accounting-analysis.json
```

The analyzer preserves missing/unsupported values, rejects malformed or duplicated
counter rows and reports ratios over counted observations. These are diagnostic
runs; overhead is not assumed negligible. Process user/system CPU, perf task-clock
and whole CPU-set busy time must stay distinct. CPU-set data includes other
processes and kernel workers; IRQ totals are not exclusive process attribution.
See the [observed accounting discrepancy](performance.md#per-thread-counter-diagnosis-2026-10-17).

Timeline plots now accept `--title` and `--subtitle`; supply the actual workload
when comparing cases. Defaults make no user/rate/worker-count claim. Ping-axis
limits derive from the recorded cohorts instead of assuming 100,000 users.


## Initial heartbeat phases

After a successful native join, the first Ping is scheduled within the configured
heartbeat interval using the existing random session ID as its phase. It may
arrive immediately after joining. Subsequent scheduled checks retain
`--ping-interval-secs`, and `--pong-timeout-secs` starts when a Ping is sent as
before. Interval zero still disables heartbeats. No additional setting or random
number read is required, and the policy adds no per-connection storage or
per-packet work.

This spreads maintenance work after a large join wave instead of placing most
first deadlines together. The measured workload shows smaller Ping cohorts and
lower median user p99, with a CPU tradeoff and some adverse latency outliers.
At 100,000 idle users, one exploratory 30-second pair uses 0.91 versus 1.51
server process CPU seconds; that is not a repeated median or total-host cost.
Keep the current 400k/s eight-worker operating recommendation and capacity
headroom. See [full comparisons](performance.md#initial-heartbeat-phase-spreading-2026-10-17).

## Longer runs with bounded per-user latency storage

Build the capacity example and use the histogram mode for sustained tests:

```sh
cargo build --release --locked --bins --example capacity
python3 scripts/capacity.py \
  --server target/release/war-relay --client target/release/examples/capacity \
  --users 100000 --seconds 600 --messages-per-second 4 \
  --workers 8 --client-workers 8 --server-cpus 0-7 --client-cpus 8-15 \
  --pin-workers --ping-interval-secs 15 \
  --tcp-send-buffer-bytes 16384 --tcp-receive-buffer-bytes 65536 \
  --timeout 900 --user-latency-histogram-output artifacts/soak-users.csv \
  --output artifacts/soak.json
```

The new per-user mode preallocates two fixed 64-bucket nanosecond histograms
before the ready gate. Each active user's histogram value occupies 1,064 bytes,
excluding its Box allocation overhead and the rest of the load generator.
Recording adds no histogram allocation as message count grows. Send-to-receive
and intended-send-to-receive histograms stay separate. CSV percentile columns
have `_upper_ms` names because they are power-of-two bucket bounds; maxima are
exact observed durations. These are instrumented end-to-end observations,
including the client, not isolated server time or exact percentile samples.

This mode requires one wave and excludes `--user-latency-output`. Only active
users have latency rows; all connected users still receive routing proofs before
and after the workload. The controller records process RSS during traffic and
logs child PIDs for observation/recovery. RSS excludes kernel socket memory.
The monitor time series grows with duration; per-user histogram storage does not.

Plot bounded outputs explicitly:

```sh
python3 scripts/plot-user-latency-comparison.py --histogram-bounds \
  --case 'Ten-minute run=artifacts/soak-users.csv' --output artifacts/soak-latency
```

Every case must use the same format. Bound-mode JSON keys and axis labels remain
distinct from exact-mode output. Optional thresholds describe histogram upper
bounds meeting the threshold; an upper bound above a threshold does not prove
the exact percentile exceeded it. Do not compare a new bounded-client run with
old exact-client timings as a relay-only speedup.

## Controlling client placement in latency studies

Client placement is part of the workload configuration. `--client-cpus` sets the
process's allowed CPU set. The existing `--pin-client-workers` additionally fixes
each Tokio runtime worker to one allowed CPU at the ready gate, using the
harness; it does not change the client executable or pin setup/main-thread work.
The harness records the singleton masks and verifies them after measurement.
`--pin-workers` independently selects native server-worker startup affinity.

For a new matched study, build once and reuse the same executable files in every
case. This example uses the current client, so its identity differs from the
older frozen client in the archived study:

```sh
cargo build --release --locked --bin war-relay --example capacity
python3 scripts/capacity.py \
  --server target/release/war-relay --client target/release/examples/capacity \
  --users 100000 --seconds 30 --messages-per-second 6 \
  --workers 8 --client-workers 8 --server-cpus 0-7 --client-cpus 8-15 \
  --pin-workers --pin-client-workers --ping-interval-secs 15 \
  --tcp-send-buffer-bytes 16384 --tcp-receive-buffer-bytes 65536 \
  --user-latency-output artifacts/client-fixed-1.csv \
  --output artifacts/client-fixed-1.json
```

Compare with the identical command omitting only `--pin-client-workers`, using
fresh CSV/JSON output paths. Run inherited/fixed/fixed/inherited order, each as
a fresh server/client process. Repeat separately at four messages/user/s for
400k/s. Keep binary hashes, active population, payload, heartbeat, CPU sets and
socket buffers identical. Use the comparison plotter for both actual-send and
intended-send user p99 distributions. Record the CPU topology too; the numeric
CPU sets above describe this machine, not a portable placement prescription.

The [selected-runtime refresh](performance.md#client-placement-refresh-and-migration-counters-2026-10-17)
favors fixed client placement in both 600k pairs but finds adverse 400k results;
an older 600k ablation was mixed. Benchmark defaults remain unchanged. Client
placement is an explicit control for investigating generator/relay/kernel
coupling, not a universal tuning rule. A generator intervention is not a
relay-code speedup, and it does not justify raising the operating recommendation.
Keep placement identical on both sides of a server optimization comparison.

## Checking responsiveness with global observers

Global broadcasts and channel/private fanout through global observers count
recipient visits and finish the selected message before yielding, preserving
their permission barriers. They do not impose a hard duration bound on one
large fanout. Ordinary rooms without global channel observers use a bounded
within-message continuation, described below.

The inspection client can place its verifying readers in the global channel as
authenticated channel-message observers. The publisher stays in its ordinary
room, alongside independent private traffic, connection churn and management
queries:

```sh
cargo build --release --locked --bins --example inspection
python3 scripts/inspection.py \
  --server target/release/war-relay --client target/release/examples/inspection \
  --mode http --rooms 64 --seconds 6 --inspectors 2 \
  --inspection-interval-ms 10 --pairs 1 --churn-workers 8 \
  --workers 4 --client-workers 8 --server-cpus 0-3 --client-cpus 8-15 \
  --fanout-readers 512 --fanout-global-observers --fanout-window 16 \
  --output artifacts/observer-mixed.json
```

This command uses allowed CPU sets, not singleton server-worker startup pinning.
The numeric sets describe the benchmark host. Observer mode requires CBOR and
cannot be combined with `--fanout-churn-every`, whose departure oracle models
readers inside the publisher's room. Independent `--churn-workers` remain
supported. Omitting `--fanout-global-observers` retains the existing ordinary-room
probe. Setup authentication, complete reader/message matrices, sender/body/FIFO,
all global-room departures and final resource/counter totals are checked.

For a server comparison, freeze the same client and run fresh baseline/candidate/
candidate/baseline cases, separately with one and four native workers. Compare
private, churn, inspection and fanout results together. Histogram percentile
columns are upper bounds; maxima are observed durations. The
[measured selection](performance.md#global-and-observer-fanout-work-quotas-2026-10-17)
improves stressed private/churn responsiveness at a fanout-throughput cost and
has explicit adverse inspection and normal-traffic observations. It does not
raise the 100,000-user operating recommendation or establish a hard latency SLO.


## Mixed population fairness and bounded room work

The selected ordinary-room path visits at most 1,024 recipients per publisher
turn. It retains one immutable membership snapshot and shared body, completes
that message before reading the publisher's next input, and yields after its
last chunk as well. The quantum bounds delivery submission; creating or refreshing
the existing cached membership view still traverses the room under the registry
read guard. It does not provide a wall-clock scheduling guarantee. A late join does not receive the unfinished older message;
retired peers cannot be redirected through reused IDs/tokens. Global/observer
permission barriers retain their existing complete-message path.

A publisher allocates its continuation box only when first needed and reuses it.
An empty box holds no payload or membership view. The source body's reservation
uses the existing per-peer/global queue budget. Retained view capacity is also
charged conservatively per active publisher to the global budget, even when
several publishers share the view. This prevents churn from pinning unaccounted
old views. `--write-timeout-secs` covers unfinished submission, and cancellation,
Close, timeout and connection teardown release its credits and references.

`war_queued_bytes` therefore includes unfinished source-body/view credits as
well as recipient delivery reservations. Shared payloads can have multiple
logical reservations; this gauge is not unique physical memory or a process RSS
cap. Reused empty continuation boxes are bounded by admitted connections.
Existing queue/message counts and aggregate decoder staging limits still apply.

Application-output-only wakes flush queued slices without entering the decoder.
Input/timer/error readiness upgrades a queued turn, and terminal/control state,
due maintenance, expired policy or shutdown uses the full state machine. This
preserves Close/partial-write and heartbeat behavior while reducing speculative
reads and repeated locking on fanout recipients.

The [mixed-load report](benchmarks/2026-10-18-mixed-100k-fairness/README.md)
combines 100,000 continuously active private users with 8,192 room readers,
32 complete churn cycles/s and two authenticated inspectors. It includes every
user's actual-send and intended-send latency, exact counters, source/binary
hashes, full retirement, quiet controls and explicit fanout-window tradeoffs.
The current-host frozen recipe is:

```sh
python3 scripts/mixed-capacity.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --scope room --pairs 0 --churn-cycles-per-second 4 \
  --fanout-windows-per-second 16 \
  --output artifacts/mixed-population.json
```

Use a new output path for each fresh trial. The helper records topology and
verifies that load clients do not share the server's physical cores; its numeric
CPU sets are specific to this machine. The report pins the exact frozen clients
and their sources. Missed sidecar timer ticks are skipped and counted by completed
versus offered windows. Every submitted window must still reach every reader,
and all scheduled private probes must complete.

For short mixed-load screens, set `--seconds 6` explicitly; setup and checked
cleanup remain outside that traffic phase. Add `--pin-client-workers` to pin
each of the two clients' eight I/O workers to a distinct CPU within its existing
mask. The helper uses the shared affinity implementation, records each thread
and CPU in `client_thread_affinity`, and rechecks affinity before, during and
after measurement. It leaves the default placement unchanged and does not pin
individual Tokio tasks. A process CPU mask alone is not per-thread pinning.
Compare both settings with otherwise identical commands: pinning can improve
fanout window timing while costing CPU, and does not guarantee better private
user tails. The [short placement comparison](benchmarks/2026-10-18-mixed-client-pinning/README.md)
records the measured tradeoff.

The inspection client supports `--external-capacity-rooms` and
`--external-capacity-room-size` to verify the stable capacity-client census.
`--fanout-close-window 32` batches departure draining outside measurement while
checking every expected notification in order; default 1 retains serial cleanup.

Departure notifications now use owner-worker multicast too. During a large
simultaneous close, bounded notification queues can hit capacity for a peer
whose normal Close is already arriving. The existing Close handling can honor
that handshake and cancel untouched queued notifications. Consequently,
`war_slow_consumers_total` counts overload intent and can rise during teardown
without representing failed live delivery. For phase-specific pressure checks,
`stress --teardown-gate` flushes `traffic_finished`, waits for a literal
`continue` line, then closes readers. The gated 1,030-client case proves exactly
eight live slow consumers before the deliberate closing-notice burst; its
healthy matrices, all eight departures and final resource retirement are checked.
