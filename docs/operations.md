# Operating the relay

`war-relay --help` lists every option; the [configuration reference](#configuration-reference)
below gives each one's environment variable, default and valid range. Secrets
are loaded from `WAR_AUTH_PASSWORD` and `WAR_ADMIN_TOKEN`; invalid encoding or
unusable HTTP admin tokens fail before binding without echoing secret values.
Empty values retain absent-secret behavior. Set configuration explicitly for
each deployment. Terms such as session, room, departure queue and staging
credit are defined in the [glossary](glossary.md).

## Endpoint and certificates

The relay serves WebTransport (HTTP/3 over QUIC) on one UDP socket, by default
`127.0.0.1:1338`, at the path `/war`. Management HTTP defaults to
`127.0.0.1:1339`. The relay terminates TLS itself: production uses a publicly
trusted certificate chain and key with `--tls-cert` and `--tls-key`, read at
startup, so restart the relay after a certificate renewal. `--dev-cert`
generates a temporary certificate for `localhost`, `127.0.0.1` and `::1` and
logs its SHA-256 pin; browsers accept it only through
`serverCertificateHashes`. [deploy/README.md](../deploy/README.md) installs the
systemd unit, which passes the certificate files as systemd credentials.

### Endpoint servicing

`--workers` controls the multi-threaded runtime that drives QUIC connections and
sessions. One additional `war-ingress` thread owns a current-thread runtime for
the shared UDP socket and endpoint dispatch. It shares the process CPU
allowance; `--pin-workers` pins the application workers and does not reserve an
extra CPU for ingress. By default (`auto`) the workers are pinned when the
process may use exactly one CPU per worker, which is the case with the default
worker count on hosts of up to 16 CPUs; the relay then only partitions CPUs it
was given. Use `--pin-workers=off` when the host shares those CPUs with other
busy work. Session shutdown completes before the ingress runtime is stopped and
joined; startup failure also stops and joins it.

Reliable private and control output attempts an immediate stream write when
there is no older output; partial writes retain FIFO position, charged credit
and their write deadline. The queue ordering lock also protects the stream and
in-flight marker; a pending write holds no lock across an await and cannot be
overtaken. Fanout stays queued for recipient batching.

The shared socket's receive buffer is requested with
`WAR_UDP_RECEIVE_BUFFER_BYTES` (4 MiB by default). Linux caps the request at
`net.core.rmem_max` and commonly reports double the request; check the logged
effective size and raise the sysctl on busy hosts, or bursts from many sessions
are dropped in the kernel and recovered as QUIC loss.

The relay's QUIC transport detects loss by the RFC 9002 time threshold only,
not by packet count, so ordinary path reordering does not collapse the
congestion window; it requests an acknowledgement every eight packets with a
5 ms maximum delay. The vendored stack's patches are described in
[vendor/README.md](../vendor/README.md).

### Browser origin admission

`WAR_ALLOWED_ORIGINS` (or repeated `--allowed-origin`) lists exact browser
origins such as `https://app.example` or `http://localhost:4180`; the literal
`null` origin can be listed too. Up to 64 entries are accepted. With a list, a
session request whose `Origin` header is absent is still admitted, because
native clients send none; an `Origin` that is not listed, or a request with more
than one `Origin` header, receives HTTP `403` and counts in
`war_origin_rejected_total`. Without a list every origin is admitted. Origin
checking is separate from relay authentication and application authorization.

### Management and authentication

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

## 100,000 sessions

Use the [100k environment](../deploy/war-relay-100k.env.example) and
[systemd override](../deploy/war-relay-100k.conf.example) as a starting profile.
The profile includes 110,000 connection slots for joining and closing headroom,
a 16 MiB UDP receive buffer request, 16 MiB maximum encoded messages and 256 MiB
global allowances for output and record staging. The room ceiling is 1,024
members. These are operating ceilings and safeguards, not preallocated memory or
a universal capacity promise. Large-room diagnostics raise that room ceiling
explicitly. Every session shares the one UDP socket, so descriptor limits do not
grow with the population. About 1.7 GiB of process memory was measured with
100,000 idle sessions joined, before message traffic; see
[performance.md](performance.md).

Both supplied deployment profiles retain large-file support: a 5 MiB image can
be sent in one CBOR byte string, with its normal routing envelope. The encoded
message limit is 16 MiB and the per-session output ceiling is 32 MiB; neither is
preallocated for idle sessions. Keep large files as bytes instead of Base64 text.
Global staging/output budgets still apply, so concurrent large transfers and
large multicast audiences must fit available credit. Payload bodies are shared
across recipients, while per-recipient pending delivery remains conservatively
charged. Use backpressure or reduce the offered rate when these budgets fill.

Choose worker count and `WAR_PIN_WORKERS` (default `auto`) for the actual host.
Keep load generators on different physical cores during comparisons, and record
SMT siblings, worker affinity and CPU topology. Multiple relay processes do not
share routing state; each process is an independent relay.

## Memory and backpressure

Idle sessions own no receive buffer. A record being received reserves twice its
declared length against `WAR_TOTAL_READ_BUFFER_BYTES` before its body is
allocated, and releases the reservation when the record is published to
routing; a refused reservation ends that session with `1002`. A zero allowance
disables that safeguard; do so only with an articulated alternative resource
plan. Published payloads retain immutable owners until their consumers finish.

### Output queues and pressure

`WAR_QUEUE_MESSAGES` and `WAR_QUEUE_BYTES` are per-session allowances;
`WAR_TOTAL_QUEUE_BYTES` is the global budget shared by every session, in-flight
body and captured recipient view. `war_queued_bytes` reports global
reservations as logical credit, not RSS; a reservation is held until the write
completes or is cancelled. A session that exhausts its own allowance is closed
with `1013`; it is the slow consumer. When the global budget is exhausted, the
message for the recipient being charged is dropped and
`war_global_pressure_total` increments. That recipient is closed with `1013`
only if it already holds at least a quarter of its own `WAR_QUEUE_BYTES`, or if
the dropped message was the reply to its own command. Senders are never closed
because of their recipients. Delivery under pressure is best-effort: later
recipients of one broadcast may miss a message that earlier recipients received.
Datagrams bypass the queue: each is handed to the recipient's connection, and
is dropped when it exceeds that recipient's maximum datagram size or the QUIC
datagram buffer is full (`war_datagrams_dropped_total`).

Departure notices use one queue per ordinary room, sixteen source-room-hashed
queues for `re_globl` departures and disconnect-observer notices, and one for
detached global/observer publications. Different rooms' observer notices have
no shared order. Notices to at most 128 recipients are delivered immediately
when the room's queue is idle. Larger ones become jobs that departure turns
advance 128 recipients at a time, rotating across queues so a large room need
not finish before another queue receives service. Live broadcasts above 1,024
recipients advance in 1,024-recipient turns owned by the source. Each job
reserves staging credit for its captured view from the global budget; a refused
reservation falls back to synchronous delivery, in order, and counts in
`war_global_pressure_total`, so that counter records refused reservations, not
lost messages. A departing peer's graceful close does not prove that recipients
got its notice; verify the expected recipient matrix. The design is in
[architecture.md](architecture.md).

## Lifecycle and deadlines

Terminal membership removal discards each room's cached recipient index instead
of maintaining it for future broadcasts. Captured fanouts keep their own owners;
reversible drain retains ordinary cache maintenance.

Handshake, join, write, keep-alive, application-idle and shutdown deadlines are
independent controls. `WAR_HANDSHAKE_TIMEOUT_SECS` bounds the QUIC handshake and
WebTransport session request. `WAR_PING_INTERVAL_SECS` sends QUIC keep-alives and
`WAR_PONG_TIMEOUT_SECS` is the QUIC idle timeout: a connection silent that long
ends without a close code. An interval of zero disables both.
`WAR_IDLE_TIMEOUT_SECS` retires joined sessions with no application input or
output (keep-alives do not count); zero preserves idle sessions. Queued writes
keep their own retention deadline.

Manual drain stops admission but allows existing sessions to run. Readiness
returns 503 while drained. An authenticated resume reverses a manual drain.
SIGINT/SIGTERM and embedding cancellation start terminal shutdown; management
cannot reopen it. `Relay::set_draining(false)` reports failure after terminal
stopping. Construct a new Relay when restarting an embedded service.

Terminal shutdown suppresses departure traffic, clears the departure queues and
releases global listener indexes before retiring sessions, avoiding quadratic
notification and index-search work. The endpoint closes every connection with
application code `1001`; sessions then have `WAR_SHUTDOWN_TIMEOUT_SECS` (ten
seconds by default) to finish before their tasks are aborted. The systemd unit's
stop deadline must cover this cleanup.

The embedding API is `Relay::new(config)`, then
`webtransport::EndpointIngress::bind(&config, server_config)` and
`webtransport::serve(endpoint, relay, shutdown)`, with
`webtransport::server_transport_config()` as the QUIC transport policy.
`admin_router(relay)` serves management. Configuration loading/validation is
fallible and should be propagated. Keep the `EndpointIngress` alive until
`serve` returns.

## Monitoring and checks

Track connections/sessions, QUIC connections, accepted/rejected/handshake-error
counts, queued bytes, staging reservations, output queue capacity, variable
bytes, write errors and timeouts, slow consumers, pressure, rate-limited and
idle-timeout closes, and the datagram counters. RSS can remain above initial
values after logical resources reach zero because allocators keep pages and
small queues stay warm for reconnects.

The server binary uses jemalloc on non-MSVC targets. It returns freed pages to
the OS gradually (after a 100,000-session population retires, resident memory
falls from about 1,670 to about 1,350 MiB within seconds), and it honours
jemalloc settings through `_RJEM_MALLOC_CONF` for hosts that need different decay
or arena policy. No setting is required.

Use [short recipes](performance-guide.md) to check the workload on its intended
host. Test graceful close, exact payloads/FIFO and final resource release. Do
not overlap compilation, profiling or multiple timed generators. Record failed
runs as failures.

## Configuration reference

Every option is a CLI flag with an environment variable. Validation runs before
the relay binds; an out-of-range value is a startup error. Sizes are bytes.

| Flag | Environment | Default | Valid range | Meaning |
| --- | --- | --- | --- | --- |
| `--bind` | `WAR_BIND` | `127.0.0.1:1338` | Socket address | WebTransport UDP listener |
| `--tls-cert` | `WAR_TLS_CERT` | unset | PEM file; requires `--tls-key` | Certificate chain served to clients |
| `--tls-key` | `WAR_TLS_KEY` | unset | PEM file; requires `--tls-cert` | Private key for the certificate |
| `--dev-cert` | `WAR_DEV_CERT` | off | Flag; conflicts with `--tls-cert` | Temporary localhost certificate whose SHA-256 pin is logged |
| `--admin-bind` | `WAR_ADMIN_BIND` | `127.0.0.1:1339` | Socket address; non-loopback requires `WAR_ADMIN_TOKEN` | Management HTTP listener |
| `--no-admin` | `WAR_NO_ADMIN` | off | Flag | Do not bind the management listener at all |
| `--udp-receive-buffer-bytes` | `WAR_UDP_RECEIVE_BUFFER_BYTES` | `4194304` (4 MiB) | Any; 0 keeps the OS default | Requested receive buffer of the shared UDP socket |
| `--udp-send-buffer-bytes` | `WAR_UDP_SEND_BUFFER_BYTES` | `0` | Any; 0 keeps the OS default | Requested send buffer of the shared UDP socket |
| `--allowed-origin` | `WAR_ALLOWED_ORIGINS` | unset (all origins) | Up to 64 entries of 1 to 1024 bytes, each an exact `http(s)://host[:port]` or `null`; repeat or comma-separate | Browser Origin allowlist; see [Browser origin admission](#browser-origin-admission) |
| `--max-connections` | `WAR_MAX_CONNECTIONS` | `10000` | 1 to 4294967295 | Admitted QUIC connections per process, including handshakes |
| `--max-connections-per-ip` | `WAR_MAX_CONNECTIONS_PER_IP` | `0` | 0 to 4294967295 | Admitted connections per client IP address; 0 disables |
| `--max-channels` | `WAR_MAX_CHANNELS` | `10000` | 1 to 4294967295 | Channels, including `re_globl` |
| `--max-channel-members` | `WAR_MAX_CHANNEL_MEMBERS` | `10000` | 1 to 4294967295 | Sessions per channel |
| `--max-message-bytes` | `WAR_MAX_MESSAGE_BYTES` | `16777216` (16 MiB) | 17 to 67108864 (64 MiB) | Largest encoded message, including the routing envelope |
| `--queue-messages` | `WAR_QUEUE_MESSAGES` | `64` | 1 to 4294967295 | Queued messages per session before it is a slow consumer |
| `--queue-bytes` | `WAR_QUEUE_BYTES` | `33554432` (32 MiB) | At least `max-message-bytes` + 10 | Per-session output byte allowance |
| `--total-queue-bytes` | `WAR_TOTAL_QUEUE_BYTES` | `268435456` (256 MiB) | At least `queue-bytes` | Global output budget shared by all sessions, bodies and captured views |
| `--receive-arena-bytes` | `WAR_RECEIVE_ARENA_BYTES` | `0` (disabled) | 0, or at least `receive-arena-page-bytes`; must fit the address space | Process-wide backing-store cap for reliable receive slabs, including retained published slices |
| `--receive-arena-page-bytes` | `WAR_RECEIVE_ARENA_PAGE_BYTES` | `65536` (64 KiB) | Positive and fits the address space | Minimum slab allocation on first use by a worker, shared across its connections |
| `--total-read-buffer-bytes` | `WAR_TOTAL_READ_BUFFER_BYTES` | `1073741824` (1 GiB) | Fits the address space; 0 disables | Shared staging allowance for records being received |
| `--max-variables` | `WAR_MAX_VARIABLES` | `256` | 1 to 4294967295 | Variables per channel |
| `--max-variable-bytes` | `WAR_MAX_VARIABLE_BYTES` | `1048576` (1 MiB) | 1 to 4294967295 | Key plus value bytes per channel |
| `--total-variable-bytes` | `WAR_TOTAL_VARIABLE_BYTES` | `67108864` (64 MiB) | At least `max-variable-bytes` | Global channel variable budget |
| `--messages-per-second` | `WAR_MESSAGES_PER_SECOND` | `1000` | Any; 0 disables | Per-session inbound message token bucket, records and datagrams together; exceeding it closes with `1008` |
| `--bytes-per-second` | `WAR_BYTES_PER_SECOND` | `16777216` | Any; 0 disables | Per-session inbound byte token bucket; exceeding it closes with `1008` |
| `--handshake-timeout-secs` | `WAR_HANDSHAKE_TIMEOUT_SECS` | `5` | 1 to 86400 | Time allowed for the QUIC handshake and WebTransport session request |
| `--join-timeout-secs` | `WAR_JOIN_TIMEOUT_SECS` | `10` | 1 to 86400 | Time allowed after the session opens to send the channel name; expiry closes with `1002` |
| `--write-timeout-secs` | `WAR_WRITE_TIMEOUT_SECS` | `5` | 1 to 86400 | Retention deadline for a queued write |
| `--ping-interval-secs` | `WAR_PING_INTERVAL_SECS` | `15` | 0 to 86400 | QUIC keep-alive interval; 0 disables keep-alives and the idle timeout |
| `--pong-timeout-secs` | `WAR_PONG_TIMEOUT_SECS` | `45` | 1 to 86400 | QUIC idle timeout; a silent connection ends without a close code |
| `--idle-timeout-secs` | `WAR_IDLE_TIMEOUT_SECS` | `0` | 0 to 86400 | Close joined sessions with no application input or output with `1001`; 0 disables |
| `--shutdown-timeout-secs` | `WAR_SHUTDOWN_TIMEOUT_SECS` | `10` | 1 to 86400 | Grace for sessions to finish after terminal shutdown begins |
| `--disable-global-broadcast` | `WAR_DISABLE_GLOBAL_BROADCAST` | off | Flag | `re_globl` broadcasts require an authenticated sender |
| `--workers` | `WAR_WORKERS` | available CPUs, capped at 16 | At least 1 | Connection and application workers (plus one ingress thread) |
| `--pin-workers` | `WAR_PIN_WORKERS` | `auto` | `auto`, `on` (bare flag, `true`), `off` (`false`); `on` needs one allowed CPU per worker | Pin each worker thread to one CPU; `auto` pins when the allowed CPUs number exactly one per worker |

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
256 MiB plus 64 MiB. Add per-session QUIC connection state (about 17 KiB per
idle session in the 100,000-session measurement), the UDP socket buffers and
the resident baseline of the process. That total must fit under the cgroup
memory limit with headroom, or the OOM killer ends the process instead of the
budgets refusing admission. Raise a budget only together with the limit that
contains it.

When enabling the reliable receive arena, also allow up to
`WAR_RECEIVE_ARENA_BYTES` of backing storage. For example,
`--receive-arena-bytes 8388608 --receive-arena-page-bytes 65536` permits up to
8 MiB of slabs, allocating a 64 KiB page lazily on a worker's first eligible
record. This is shared across connections; it does not preallocate 64 KiB for
every session. Workers reuse released prefixes and append into unused page space.
The final slice can return a completed slab to a single spare-page cache on its
worker, so the next allocation can reuse its backing storage. Both active and
spare pages count against the same process cap; no global pool lock is used.
Each published slice retains its page until its final consumer releases it.
The arena budget includes those retained pages and worker caches. Once that cap
is reached, or a record exceeds the page size, the reader uses its ordinary
budgeted allocation path. Arena pressure therefore does not reject otherwise
admissible messages. Message-size, staging, queue, rate and timeout limits still
apply. Datagram payload allocation is unchanged.

`war_receive_arena_reserved_bytes` and `war_receive_arena_peak_reserved_bytes`
report backing reservations separately from record staging. Arena storage may
remain cached after connections retire and is released when the worker runtime
exits; its reservation must remain within the configured cap. A larger page can
reduce payload allocations but lets one retained small slice pin more capacity.
Choose the page size and cap using actual message sizes and slow-recipient load.

After a load test, check that these gauges have returned to zero:
`war_connections`, `war_sessions`, `war_quic_connections`, `war_queued_bytes`,
`war_variable_bytes`, `war_read_buffer_reserved_bytes` and
`war_output_queue_capacity_bytes`. A gauge
that stays non-zero after every client has closed is a leak, not warm capacity.
