# WebSocket Application Relay — Rust / CBOR

A small, bounded relay for applications and multiplayer games. Clients join a
channel, receive a 64-bit session ID, and send CBOR values to another ID or their
channel. Applications own message meaning and state.

This is a behavioral port of [Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay)
at commit `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`, with the requested CBOR redesign.
The original C++ source, JavaScript clients, dependencies, history, and MIT license
remain available. The original README is in [docs/upstream-README.md](docs/upstream-README.md).

For the selected implementation, short test recipes, measurement limits and
recent rejected alternatives, start with the [current performance guide](docs/performance-guide.md).

## Native Rust fanout

Native multicast shares immutable payloads, sends on the sockets' owning workers,
and uses a small borrowed writer for a single queued frame, with bounded vectored
batching for larger queues. Ordinary private traffic
keeps its immediate-send path. Large ordinary rooms now submit at most 1,024
recipient visits per source turn, retaining the same shared body and immutable
membership view until that message finishes. Output-only wakes avoid entering
the input decoder; departure notifications use the same owner-worker path.

Large room and disconnect-observer retirements use one charged FIFO, advanced
in 1,024-recipient turns outside the registry lock. This preserves departure
sequence across workers while allowing unrelated routing to proceed. The
[short 100,000-recipient departure comparison](docs/benchmarks/2026-10-19-departure-fifo/README.md)
reduces the worst observed witness RTT from 107.13 to 4.57 ms, with ordinary
mixed-load CPU effectively flat. Queue-pressure fallback remains synchronous;
its ordering and memory limits are covered separately in that report.

Workers also discover at most 1,024 expired timers per turn before returning to
ready processing and socket polling. The [expiry-burst report](docs/benchmarks/2026-10-19-expiry-fairness/README.md)
records the responsiveness improvement beside 100,000 overdue pending handshakes,
normal mixed-load checks and deadline/shutdown regressions.

The [100,000-user mixed-load fairness report](docs/benchmarks/2026-10-18-mixed-100k-fairness/README.md)
records the selected runtime, every user's latency, bounded-memory/FIFO checks
and comparisons with previous Rust and modern C++. Median user p99 falls from
11.99 to 2.03 ms under the measured competing load; fanout-window tails and the
small quiet-path CPU cost are reported explicitly. The earlier
[fanout throughput study](docs/benchmarks/2026-10-17-rust-owner-fanout/README.md)
records the preceding owner-worker transport change.

The [short multicast experiments](docs/benchmarks/2026-10-18-buffered-fanout/README.md)
add exact recipient latency and intended-send measurements. Buffered batches cut
CPU and broadcast delay, but did not establish consistent private-user tails;
the current runtime remains selected. The longest measured phase was 30 seconds.

## Modern C++ comparison candidate

A separate [C++20 static library and relay](cpp/README.md) now implements the
native CBOR protocol on pinned modern uWebSockets/uSockets. It uses per-worker
socket ownership, reusable bounded delivery batches and borrowed local payloads.
See the [measured comparisons](cpp/BENCHMARKS.md) for throughput, CPU, memory,
per-user latency, validation and remaining operational differences. The selected
Rust runtime and the original C++ implementation remain independently available.

## Run

Requires Rust 1.91 or newer; no C++ libraries, OpenSSL, TBB, or libuv are needed.

```sh
cargo build --release --bins --example load
./target/release/war-relay
```

Defaults: `ws://127.0.0.1:1338` for CBOR and `http://127.0.0.1:1339` for local
management. `--help` lists all limits and environment variables. Set
`WAR_AUTH_PASSWORD` to enable privileged relay commands and `WAR_ADMIN_TOKEN` to
enable management operations. Both are disabled when absent; the original
hardcoded password is removed. Secrets are environment-only and never logged.
Secret values must be valid UTF-8; invalid encoding stops startup with the variable
name, without echoing its value. Empty values retain the absent-secret behavior.
Admin tokens must also be valid HTTP field values without trailing ASCII spaces or tabs; unusable values fail validation before binding without echoing the token. Relay passwords retain their existing rules. See the [HTTP transport checks](docs/benchmarks/2026-10-19-admin-token-transport/README.md).
Rust embedders should propagate `config.load_secrets()?` before validation. See
the [startup encoding checks](docs/benchmarks/2026-10-19-config-secret-encoding/README.md).

Optional `--max-connections-per-ip 32` admission bounds concurrent TCP sockets
from each direct peer address, including unfinished upgrades. Both native and
legacy ingresses support it; the default `0` preserves unrestricted per-IP
admission. See the [address and proxy model](docs/operations.md#direct-peer-connection-admission)
before enabling it behind a proxy or bridge.

Native sockets run on readiness workers with shared receive storage and direct
vectored output. Idle sockets own no receive slab or message channel. A worker
keeps one base-size slab; partial frames retain their own storage, and oversized
completed slabs retire before reuse by another connection. Published payloads
keep their immutable owner. Synchronous output borrows the frame and reservation
owners; deferred output retains owned handles to the same body. Byte reservations
are charged before writing and held until completion or cancellation.
`--read-buffer-idle-secs` additionally controls quiet
decoder/fragment high-water reclamation. The storage hooks and source provenance
are documented in [vendor/README.md](vendor/README.md).

The [100,000-user operating profile](docs/operations.md#100000-native-connections)
adds TCP/admission headroom, descriptor and memory settings, kernel buffer
controls and reproducible real-socket acceptance. The [measured workloads](docs/performance.md#100000-connected-users-2026-10-16)
cover 100,000 connected users in rooms of 16, full checked 1 KiB private messages,
heartbeats, reconnect churn, unread-recipient isolation and complete retirement.
A [ten-minute per-user soak](docs/performance.md#bounded-per-user-latency-and-ten-minute-soak-2026-10-17)
verifies 240 million application messages with bounded histogram retention;
its finite loopback scope and histogram bounds are reported explicitly.
[Global/observer fanout accounting](docs/performance.md#global-and-observer-fanout-work-quotas-2026-10-17)
charges recipient visits to the native turn budget and retains complete-message
permission barriers in those scopes. Ordinary large rooms without global channel
observers instead use the [bounded within-message continuation](docs/benchmarks/2026-10-18-mixed-100k-fairness/README.md).
Measured responsiveness, fanout costs and validation scope are explicit.
Native `--tcp-send-buffer-bytes` and `--tcp-receive-buffer-bytes` configure kernel
buffers independently of application queues; their default `0` retains OS policy.

Initial heartbeat deadlines spread across the configured interval to avoid
concentrating maintenance work after a join wave. Recurring cadence and Pong
timeout protection remain configured independently.

The design prioritizes consistently low response latency for individual users,
including tail latency and fairness across the connected population. Peak
throughput is a secondary tradeoff: a measured bandwidth or throughput reduction
can be worthwhile when it improves responsiveness while preserving mass-scale
capacity, correctness and bounded memory. The [per-user latency measurements](docs/performance.md#per-user-latency-fairness-at-100000-connections-2026-10-17)
and [temporal diagnostics](docs/performance.md#latency-timeline-and-observed-heartbeat-cohorts-2026-10-17)
record those criteria alongside CPU, memory and completed work.

After large connection populations retire, sparse worker arenas compact while
surviving clients stay connected. Socket tokens and shared payload storage stay
stable; small arenas remain allocated for ordinary reconnect traffic. See the
[scaling policy](docs/operations.md#scaling-boundary) for thresholds and costs.

Operators can optionally retire inactive joined sessions with
`--idle-timeout-secs 300` (`WAR_IDLE_TIMEOUT_SECS=300`). Binary application input
and outgoing relay messages refresh activity; receive-only consumers stay active
while messages arrive. Ping/Pong traffic does not refresh this deadline. Expiry
closes with `1001` / `Application Idle` and uses normal queue/variable cleanup.
The default `0` disables this policy. See [operations.md](docs/operations.md) for
memory budgeting and deadline details.

For a browser application, optionally restrict HTTP upgrades with
`--allowed-origin https://app.example` or `WAR_ALLOWED_ORIGINS=https://app.example`.
Repeat the option or comma-separate values for multiple sites. Origins match
exactly; use the page's `location.origin`, without a trailing slash. Unlisted
browser origins receive HTTP `403`. Missing Origin remains allowed for native
clients, so this is a browser boundary, not client authentication. With no list,
the original unrestricted behavior remains. Apply the same setting to an exposed
legacy bridge. See [origin admission](docs/operations.md#browser-origin-admission)
for proxy behavior, `null` origins, and telemetry.

## JavaScript client

Install with `npm ci --prefix clients/js --ignore-scripts`. The client supports
modern browsers through an ES module bundler and Node 22+. IDs are `BigInt`.
Received IDs are checked for exact integer representation and uint64 range;
welcome, departure and assigned session IDs also exclude reserved values.
WebSocket welcome and data frames must be binary. Text frames are rejected before
byte-view construction, avoiding numeric-string allocation coercion; the
[binary-wire regressions](docs/benchmarks/2026-10-19-js-binary-wire/README.md) cover
welcome, pending controls and queued commands. Application string values travel
as CBOR inside binary frames.
Wire envelopes and control reply bodies must be two-element arrays; arbitrary
application values retain their CBOR types. The [response-shape check](docs/benchmarks/2026-10-18-js-response-shapes/README.md)
covers byte-string lookalikes and missing/extra fields.
Variable results require `[found: boolean, value]`; authenticate and
set-variable results require booleans. Listener results are effective integer flags
in the range 0 through 7. The [command-result checks](docs/benchmarks/2026-10-18-js-command-results/README.md)
preserve valid false replies and arbitrary stored CBOR values.
Channel listings require `[name_bytes, population]` rows with 0–16-byte names
and exact nonnegative uint64 populations; valid Number/BigInt values are preserved.
The [listing checks](docs/benchmarks/2026-10-18-js-channel-listing/README.md)
include binary/empty names and a real native management query.
Malformed responses immediately reject any pending command, then close with
application error `4002` (permitted by the browser WebSocket API); rejection does not wait for the peer’s Close acknowledgement. The [short validation
and compatibility check](docs/benchmarks/2026-10-18-js-wire-id/README.md) records
invalid-ID regressions, valid large-ID routing and the measured client cost.
The [listener compatibility checks](docs/benchmarks/2026-10-18-js-listener-compatibility/README.md)
exercise actual native acknowledgements and real WebSocket error closes. Server-sent
protocol Close `1002` is preserved.
An assignment acknowledgement updates `userId` before subsequent message callbacks;
`assignId()` still resolves to the acknowledged BigInt ID. The
[assignment-order regression](docs/benchmarks/2026-10-18-js-assignment-order/README.md)
checks pipelined assignment and self-routing against the native relay.
Connection setup removes its temporary listeners on success, error, or timeout.
A failed join send rejects immediately and closes the socket; delayed events
cannot start a subscription after setup has failed. See the [short connection
lifecycle checks](docs/benchmarks/2026-10-18-js-connect-cleanup/README.md).

```js
import { Relay } from './clients/js/relay.mjs';

const a = await Relay.connect('ws://127.0.0.1:1338', 'lobby');
const b = await Relay.connect('ws://127.0.0.1:1338', 'lobby');
b.addEventListener('message', ({ detail: { sender, value } }) => {
  console.log(sender, value);
});
a.broadcast({ kind: 'hello', text: 'Hello π', data: new Uint8Array([1, 2, 3]) });
a.send(b.userId, { kind: 'private', position: [1.5, 2, 3] });
await a.setVariable('settings', { round: 1, ready: true });
console.log(await b.getVariable('settings')); // { found: true, value: ... }
a.close();
b.close();
```

`Relay.connect()` captures the supplied channel before asynchronous socket setup.
A byte-array channel can be reused or its buffer transferred after the call;
the subscription retains the original bytes, including a subarray's exact range.
The existing CBOR subscription frame is prepared once, with no extra payload copy.

`authenticate(password)`, `listen(flags)`, `channels()`, and `assignId(bigint)`
expose privileged commands. `disconnect` events carry `{ userId }`; `close` events
carry `{ code, reason }`. The client serializes control requests, including
concurrent variable requests, because v1 deliberately has no request-ID field.
Command values are captured when the method is called, so ordinary byte buffers
and object fields can be reused while replies are pending. The prepared CBOR
frame still waits for its FIFO turn; local encoding failures reject in that turn
without sending a packet. Ordinary calls encode once without another payload
copy. A command invoked reentrantly from a value getter uses `structuredClone`
and deferred encoding to avoid reentering the shared codec, so such values must
be cloneable. Shared-memory buffers remain shared under that API; keep their
contents synchronized and stable until the request settles.

Direct recursive serialization from a value getter is rejected locally:
`send()`/`broadcast()` throw, and `connect()` rejects before opening a socket.
This prevents the shared codec from producing a corrupt outer packet and leaves
the connection usable after handling the error. Queueable commands retain the
clone-and-defer behavior described above.

Calling `close()` immediately rejects an outstanding command and prevents queued
commands from sending; it does not wait for the peer's Close acknowledgement.
When the socket's final close event arrives, the client removes its own socket
handlers before notifying Relay close listeners. Caller-installed socket listeners
remain attached.
If a command's `WebSocket.send()` throws, that command keeps the original
thrown error and an otherwise-open connection closes with code 4002 and reason
`Relay Command Send Failed`. Queued commands are not sent; reconnect before
retrying. This prevents an already-forwarded command's late reply from being
assigned to the next command. Encoding failures before sending remain local
and do not force a reconnect.
`requestTimeoutMs` defaults to 10,000 and must be positive, finite, and at most
2,147,483,647 ms, the native timer delay ceiling. It is checked before connecting
and before each queued command starts, including after changing the property.
Variable replies preserve the distinction between a missing entry (`[false, null]`)
and a stored null (`[true, null]`). A missing-entry reply carrying another value
rejects the request and queued commands and closes with code 4002; see the
[reply-validation checks](docs/benchmarks/2026-10-19-js-missing-null/README.md).
A variable-write acknowledgement must be `true`; `false` triggers the same
protocol-error cleanup. Authentication may legitimately return `false` without
closing the connection. See the [acknowledgement checks](docs/benchmarks/2026-10-19-js-missing-null/write-ack/README.md).

To build a standalone browser ES module and open its acceptance page:

```sh
npm run build:browser --prefix clients/js
python3 -m http.server 4180 --bind 127.0.0.1 --directory target/browser
```

Open [the browser check](http://127.0.0.1:4180) with the native relay running, then
select **Run browser check**. The page imports the generated `relay.js` and checks
typed broadcast/private delivery, a 128 KiB byte string, concurrent variable
requests, BigInt precision, disconnect notification, and clean Close handshakes.
It opens three temporary sessions and releases them after success or failure.
The bundle targets ES2022; serve `target/browser/relay.js` with your own app to
import `{ Relay }` without a browser import map or Node polyfills.

## One protocol

The primary server accepts binary WebSocket messages containing CBOR. Offer the
`war.cbor.v1` subprotocol. Connections without an offered subprotocol also use
CBOR; unsupported offered subprotocols are rejected.

1. Send a CBOR text/byte string containing a channel name (0–16 decoded bytes).
2. Receive a CBOR unsigned integer containing the session ID.
3. Send `[target_id, value]`. Recipients receive `[sender_id, value]`.

Target `0xffffffffffffffff` broadcasts to the channel, excluding the sender.
Target `0` addresses relay commands. Other IDs route privately across channels,
including self. Unknown IDs are dropped. A departing member generates
`[0xffffffffffffffff, departed_id]` to its peers.

Values can combine text, bytes, arrays, maps, numbers, booleans, null, undefined,
and tags. The relay accepts definite-length CBOR, bounds validation to 64 nested
levels and 65,536 items, and preserves encoded payloads verbatim. It does not
materialize object trees or convert objects to JSON. See [docs/protocol.md](docs/protocol.md).

Welcome IDs and small control replies use the frame's inline prefix storage.
Control headers wrap encoded bodies directly. Variable reads share the stored
value with queued output, so replacing a variable or removing its channel keeps
an in-flight reply valid without copying the value into another message buffer.

Forwarding keeps the upstream's zero-copy intent: a validated application value
stays in its received buffer, shared across recipient queues. Routing advances
the owned input view to the value and transfers that owner directly to the
outgoing frame. The server builds only the small replacement sender header and
writes headers plus the borrowed body with vectored I/O. Changing a CBOR ID's
encoded width does not shift the body. Tiny slices of much larger receive storage
are compacted deliberately to avoid pinning that storage in a slow queue. This
concerns user-space forwarding;
WebSocket unmasking, fragmented-message assembly, and kernel transfers still do
their required work. See [docs/performance.md](docs/performance.md).

After backpressure clears, output queues retain up to 32 slots for reuse and
release larger empty allocations. The current and peak queue-capacity metrics
make that storage visible separately from reserved message bytes.

Native decoder staging has a shared 1 GiB allowance, configurable with
`--total-read-buffer-bytes` / `WAR_TOTAL_READ_BUFFER_BYTES` (`0` disables).
Frame and fragment storage reserve twice the requested capacity before growth,
retaining headroom for reuse. Intermediate fragment growth stays within the
message ceiling; final growth uses the known complete length. Exhaustion returns
HTTP `503` before upgrade or a best-effort WebSocket `1013` before refused
decoder growth. Frame capacity is admitted before reading its body; fragment
assembly is admitted before appending the decoded frame. This preserves shared
payload ownership and avoids per-packet budget atomics within the high water.
It is a staging allowance, not an RSS limit; see [operations](docs/operations.md)
for the accounting boundary, reclamation and metrics.

## Original clients

The optional migration bridge translates the original binary/text wire format:

```sh
./target/release/war-legacy-bridge \
  --bind 127.0.0.1:1337 --upstream ws://127.0.0.1:1338
```

After the frontend WebSocket upgrade, send its channel name within ten seconds.
Otherwise the bridge sends `1008 / Channel Join Timeout`, matching the native
relay's join policy. Close acknowledgement has its own bounded ten-second budget;
the admission slot is released when that exchange finishes or its budget expires.
An unjoined client opens no upstream connection. See the [join timeout and
single-slot recovery check](docs/benchmarks/2026-10-19-bridge-join-timeout/README.md).

The bridge checks upstream URL syntax and the `ws`/`wss` scheme before binding.
Rust embedders can use `legacy_bridge::validate_upstream` and
`legacy_bridge::validate_connection_limits` before binding; serving repeats these
checks. The CLI checks connection limits before binding or logging that it is
listening; see the [startup-boundary regressions](docs/benchmarks/2026-10-19-bridge-prebind-limits/README.md). Reachability and TLS availability are checked when forwarding a client.
If that connection fails or exceeds its ten-second deadline, the legacy client
receives a bounded `1013 / Upstream Unavailable` Close. A later connection attempt
can succeed once the upstream returns; see the [failure and recovery checks](docs/benchmarks/2026-10-19-bridge-upstream-unavailable/README.md).
A failed subscription write, upstream TCP drop before welcome, or welcome timeout
also produces `1013 / Upstream Unavailable`; the subscription write shares the
welcome phase’s ten-second deadline. The closing handshake has its own bounded
budget. See the [welcome failure and single-slot recovery checks](docs/benchmarks/2026-10-19-bridge-welcome-failure/README.md).
While awaiting the upstream welcome, Ping/Pong control frames are handled within
the same ten-second deadline. An upstream Close is relayed with its original code
and reason even before a legacy ID is assigned, so a full native room reports a
clean `1013 / Try Again Later` instead of an unexplained `1006` disconnect.
Frontend Ping/Pong and Close/EOF are serviced while establishing the upstream
connection and awaiting its welcome, while no application message is retained.
Leaving during a stalled upstream HTTP upgrade cancels that connection attempt
and releases admission; see the [upgrade cancellation and recovery checks](docs/benchmarks/2026-10-19-bridge-connect-cancellation/README.md).
One pipelined message may be kept across both setup phases without copying its
payload; further frontend reads pause until welcome to preserve FIFO and bounded
buffering. A later Close behind that message can still wait through the connection
and welcome deadlines. Terminal cleanup releases this retained message before
waiting for Close acknowledgement; see the [requested-heap check](docs/benchmarks/2026-10-19-bridge-pending-retention/README.md).
See also [frontend retirement and FIFO checks](docs/benchmarks/2026-10-19-bridge-frontend-welcome/README.md).
A welcome must contain exactly one CBOR unsigned uint64 ID other than zero or
the broadcast sentinel. Invalid welcomes close both peers with `1002 / Invalid
Upstream Welcome`; valid noncanonical integer widths remain accepted.
Oversized frontend messages receive Close `1009` before or after joining; the
bridge retains its 16 MiB incoming message limit. Malformed framing receives
`1002`, and invalid UTF-8 receives `1007`; decoder-error Close output is bounded
and does not resume the invalid input stream. See the [frame-error checks](docs/benchmarks/2026-10-18-bridge-frame-errors/README.md).
Both bridge legs limit fragment parsing to 256 frames per stream poll, preserving
partial messages while yielding between long continuation trains. The
[upstream fragmentation checks](docs/benchmarks/2026-10-19-bridge-upstream-fragments/README.md)
cover both welcome and forwarding.
Malformed upstream frames also produce the mapped legacy Close before or after
welcome; the [upstream fault checks](docs/benchmarks/2026-10-19-bridge-upstream-frame-errors/README.md)
verify acknowledgement, retirement and single-slot reuse.
The [short oversized-header
check](docs/benchmarks/2026-10-18-bridge-oversize/README.md) verifies rejection
without allocating a large payload and confirms independent native routing.

Existing `relay.js` clients connect to `ws://127.0.0.1:1337`. Their binary/text
payloads become CBOR byte/text strings and interoperate with native CBOR clients.
Other structured CBOR values require a CBOR client: an incompatible legacy
recipient closes with `1003`, rather than receiving an invented encoding.
Rejected upstream message owners are released before waiting for Close acknowledgement;
[fragmented versus shared-storage checks](docs/benchmarks/2026-10-19-bridge-rejected-owner/README.md)
verify the memory-lifetime benefit without changing successful forwarding.

The bridge preserves original command/reply layouts and suppresses CBOR command
acknowledgements. Tests execute the **unchanged upstream `relay.js`** on real
sockets. The old `relay.module.js` remains as source history; its existing
undefined-variable and const-reassignment bugs are not copied into the new client.
An isolated check also runs the **unchanged original C++ executable** and compares
48 shared wire behaviors against the bridge, while asserting eight deliberate
behavior differences. See [compatibility.md](docs/compatibility.md) for the
recorded results and local dependency setup.

## Operating the relay

- Connection admission includes TCP handshakes, with handshake and join deadlines.
  Channel populations and inbound message/byte rates have explicit limits.
- Each recipient has a bounded message queue and byte budget, including an
  in-flight write. Routing never awaits a consumer; overflow closes it with `1013`.
  Pending writes observe session cancellation and release their reservations promptly.
- Process-wide queue and variable byte budgets bound aggregate retained payloads
  as the number of users and channels grows. Workers share 4 KiB receive chunks;
  incomplete input retains separately admitted staging.
- Channel variables have count and byte limits and disappear with an empty
  ordinary channel. `re_globl` persists, matching the original behavior.
  Peers share the registry's canonical channel-name storage; a new channel owns
  one bounded name copy independent of receive buffers.
- OS entropy supplies IDs. Fixed-ID takeover closes the old owner with `4001`;
  old-session cleanup cannot erase its replacement. Reserved IDs cannot be claimed.
- Mio workers drive native sockets; Tokio handles management and process signals.
  HTTP upgrade and live decoder metadata share inline connection storage;
  resuming a partial upgrade reuses that storage without a state-box allocation.
  Native accept work yields after at most 16 sockets per turn. Each worker
  keeps at most one pending accept continuation, so repeated
  listener events cannot multiply queued admission batches. Both public CLIs
  accept `--listen-backlog` / `WAR_LISTEN_BACKLOG` (default 1,024), applied
  consistently to native listeners and subject to the OS cap. See
  [pending TCP connections](docs/operations.md#pending-tcp-connections).
  Ordinary private routing submits bounded nonblocking writes directly.
  Multicast submits shared frames to socket owners, which drain bounded vectored
  batches without waiting to fill them. Queues reuse their slots; private traffic
  creates a queue only under backpressure. A per-socket lock serializes immediate
  and owner writes; registry reads protect routing and writes protect membership/ID changes.
  Private routing with no eligible observers retains the target under the
  registry lock, then releases that lock before output. Global listener indexes
  visit opted-in sessions for channel, private and departure events.
  Fanout shares one immutable application body across recipients.
  Ordinary room broadcasts without global channel observers capture a reusable
  view of peer handles and release the registry before output. Their read turns
  account for recipient work and yield between complete messages. Membership
  changes update an exclusively owned view in place, or invalidate a captured
  view while its routing owners finish. Application bodies never enter that cache.
- SIGINT/SIGTERM stop admission and enforce a shutdown deadline. Responsive peers
  receive `1001`; blocked transports can be dropped. Peer-initiated Close frames
  receive their exact acknowledgement. Dropping the service signals its workers
  to retire. Untouched queued writes permit a normal close handshake; a partly
  transmitted frame requires dropping its transport.
  Process shutdown flushes its `1001` Close before TCP retirement and does not
  wait for a peer acknowledgement. Service cleanup keeps joining remaining
  workers after a task failure, preserving the first error within the existing
  shutdown deadline.
- OS resource pressure during accept, including an exhausted descriptor limit,
  suspends new admission for one second while established sessions continue. Both ingresses
  retry and recover queued connections. Native failures are exposed through
  `war_accept_errors_total`; see [resource-pressure behavior](docs/operations.md#accepting-under-os-resource-pressure).
  Native TCP configuration or readiness-registration failures retire the affected
  socket, preserve its admission permit until TCP closes, and use the same retry
  delay on that worker. `war_setup_errors_total` also counts these refusals.

Management uses a separate HTTP listener:

| Endpoint | Purpose | Authorization |
| --- | --- | --- |
| `GET /healthz` | Liveness | None |
| `GET /readyz` | `503` while draining or stopping | None |
| `GET /metrics` | Prometheus counters/gauges | Bearer token when configured |
| `GET /v1/channels` | Populations and variable usage | Bearer token required |
| `POST /v1/drain` | Reject new connections/joins | Bearer token required |
| `POST /v1/resume` | Resume a manual drain; `409` after shutdown starts | Bearer token required |
| `POST /v1/sessions/{decimal_id}/disconnect` | Close session with `4000` | Bearer token required |

On a matched session-disconnect route, missing or invalid credentials return
`401` even when the ID is malformed. Authorized malformed IDs retain `400`;
valid IDs without a live session return `404`.
Session IDs retain full uint64 precision in the decimal URL path. JavaScript
management tools should keep IDs as `BigInt` and use `id.toString()` to build the
path. [Live-session boundary checks](docs/benchmarks/2026-10-19-admin-id-boundaries/README.md)
verify exact targeting and neighbour liveness across the 32-bit, JavaScript integer
precision and maximum-session-ID boundaries.

Manual drain preserves existing sessions and can be resumed. Shutdown is terminal:
readiness and admission remain drained even if a management request races with
worker shutdown. Rust embedders use `Relay::is_draining()` and
`Relay::set_draining(bool)` in place of the former public atomic field; resuming
a stopping relay returns `false`. Restart with a new `Relay` instance.

For bounded channel browsing, use `GET /v1/channels?limit=100`. Pass the last row's
URL-encoded `name_base64` as `after` to continue. Limits are 1–1,000; the JSON row
format and base64 ordering match the full listing. Each page is a fresh coherent
view. See [paging and concurrent changes](docs/operations.md#bounded-channel-browsing).

Non-loopback admin binding requires a token. Liveness/readiness expose no session
information; metrics use no per-user/channel labels. See [docs/operations.md](docs/operations.md)
and [deployment examples](deploy/).

The original `re_globl` channel can broadcast to all users, and authenticated
listeners can observe selected channel/private/disconnect traffic. These original
features do not provide tenant isolation. `--disable-global-broadcast` requires
authentication for global sends. Use separate processes for untrusted applications;
application authentication/authorization belongs in the application or admission proxy.

This is a single-node relay: IDs, channels, variables, and private routing are
process-local. Replicas do not form a cluster. Cross-node routing and durable
history are not implemented. Capacity claims require measurements on the intended
host, network, payloads, and fanout.

## Verify and measure

```sh
cargo fmt --check
python3 scripts/check-vendor.py
python3 scripts/test-build-snapshot.py
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
./scripts/check-js.sh
npm run build:browser --prefix clients/js
```

The codec source audit uses Python 3.12+ and Git, caching two checksum-pinned
published archives on its first run. Rust builds use the included codec sources.

The JavaScript script launches the executable paths reported by Cargo, including
when `CARGO_TARGET_DIR` points outside the checkout. It runs the full CBOR and
original-client suite, including idle Close propagation, against one disposable
native/bridge fixture. Before shutdown it requires connections, sessions, queued
bytes, variable bytes, read-buffer reservation and output-queue capacity to return
to zero. Its exit trap removes both services on success or failure. The fixture
uses loopback ports 19137–19139, four native I/O workers and four bridge runtime
workers; those ports must be available. Test execution takes about a second on the measured host; compilation
and dependency installation add time.
CI also builds the browser bundle; browser execution is a separate acceptance
step using the page above.

To repeat the original C++ runtime comparison on Linux/x86-64, with Python 3.12+,
`ar`, and working `bwrap` network namespaces:

```sh
cargo build --locked --release --bins
python3 scripts/reference.py --output artifacts/reference/report.json
```

Its pinned libraries are cached locally and all test listeners run in a private
network namespace. It leaves host services and system libraries in place. This
optional comparison is separate from the standard CI checks.

For a performance comparison against that preserved C++ executable, using the
same verified application workload directly on each native protocol:

```sh
cargo build --locked --release --bins --example inspection
python3 scripts/compare-cpp.py --workers 1 --server-cpus 0 \
  --output artifacts/cpp-one-core.json
python3 scripts/compare-cpp.py --workers 4 --server-cpus 0-3 \
  --output artifacts/cpp-four-core.json
python3 scripts/compare-cpp.py --rebuild-cpp --workers 4 --cases private query \
  --output artifacts/cpp-rebuilt.json
```

Private delivery, private delivery with reconnect churn, and private delivery
with authenticated channel queries run as separate cases. C++ uses its original
binary wire format; Rust uses CBOR. The original shipped binary's build flags
are unknown. Its delayed retirement semantics, actual thread counts, CPU use,
sampled RSS and every checked lifecycle are recorded. Run comparisons separately
from compilation, profiles and other load tests. This is a comparison of those
complete servers and protocols on loopback, not an isolated language benchmark.
The optional rebuild needs Clang plus OpenSSL/TBB/libuv/zlib development files.
It uses upstream optimization flags and a temporary copy with only the worker
count fixed, then verifies every C++ listener binds. The preserved shipped
executable starts only one listener here. Rebuilt four-worker churn failed
verification with a repeated welcome ID; its failed diagnostics are separate
from successful timings. See the
[readiness runtime comparison](docs/performance.md#readiness-runtime-rewrite-2026-10-16)
for the current native build, the retired Rust control, C++, exact scope, memory
costs and limits. `--control /path/to/frozen/war-relay` adds the retired async
runtime to the same comparison using its per-connection staging contract.

To measure concurrent inspection within Rust, including its HTTP management
endpoint, use `python3 scripts/inspection.py --mode http --output
artifacts/inspection.json`. Modes `none` and `cbor` provide companion controls;
the default workload holds 8,192 idle rooms alongside private traffic and churn.
All inspection responses and application deliveries are checked.

The verified client supports up to 64 private pairs. `--client-workers 8`
selects eight client runtime threads; the default remains four. Give the client
separate CPU cores when measuring a larger active population. For example, on
a host with physical cores 0–11 available:

```sh
python3 scripts/compare-cpp.py --rebuild-cpp --workers 4 --pairs 64 \
  --client-workers 8 --server-cpus 0-3 --client-cpus 4-11 \
  --cases private query --output artifacts/cpp-64-pairs.json
```

The inspection harness accepts the same `--pairs` and `--client-workers`
options. Omit `--client-workers` when replaying an older frozen client that
predates the option. The [64-pair comparison](docs/performance.md#forwarding-counter-experiments-and-64-active-pairs-2026-10-16)
records full verification and CPU/latency/memory results; a larger active
population does not by itself establish a saturated server capacity.

To check receive-slab retention under actual unread-recipient backpressure:

```sh
cargo build --release --bins --example retention
python3 scripts/retention.py --cycles 64 256 --output artifacts/retention.json
```

The runner starts fresh loopback servers with rates/heartbeats disabled and a
60-second write deadline. Eight 1 MiB deliveries block one recipient, then each
cycle sends a 4 MiB unknown-target primer, a tiny queued value, and a verified
self-message barrier. After a two-second hold, it verifies all buffered output,
both Close handshakes, counter totals, and complete cleanup. It samples server
RSS and logical queue bytes; these are finite retention checks, not a leak proof
or a production capacity estimate.

Add concurrent producers and observe connected-idle storage after draining:

```sh
python3 scripts/retention.py --cycles 64 --producers 1 8 32 --idle-secs 2 \
  --output artifacts/concurrent-retention.json
python3 scripts/retention.py --cycles 64 --producers 1 8 32 --idle-secs 2 \
  --server-idle-secs 3 --output artifacts/concurrent-idle-expiry.json
```

Producers start together; the recipient verifies each sender's sequence without
requiring a global order. Preparation is shared and bounded; each case permits
at most 8 GiB of configured primer payload, plus wire headers. The runner sizes
the message-count queue to fit the producer matrix while retaining the default
32 MiB byte budget. Connected-idle observations include two seconds of sampling
after all sessions retire. The second command requires every session to receive
and acknowledge the configured server-idle Close.

```sh
./target/release/examples/load --clients 128 --messages 1000 --payload-bytes 256
./target/release/examples/load --mode broadcast --clients 64 --messages 500
python3 scripts/benchmark.py --output artifacts/read-buffer.json
cargo build --release --example stress
python3 scripts/stability.py --output artifacts/stability.json
```

Choose a fresh stability output path for each run. The runner refuses existing
paths before launching a relay and retains metadata, completed cases and the
error if a workload raises.

To check private routing before, during and after an idle global-room population:

```sh
cargo build --release --bins --example registry_peak
python3 scripts/registry-peak.py --scope private --measure-populated \
  --peak-clients 1024 --seconds 3 --output artifacts/private-population.json
```

The probe checks every payload and sender with a 16-message window, then verifies
every ordered departure notification, Close and EOF. Larger shared populations
take longer to retire because the protocol sends each departure to every
remaining member; routing timing excludes setup and retirement.

To check admission against concurrent incomplete frames while a healthy session
continues routing, use a deliberately smaller staging allowance:

```sh
cargo build --release --bins --example input_budget
python3 scripts/input-budget.py --server-arg=--total-read-buffer-bytes=25165824 \
  --expect-admitted 1 --output artifacts/input-budget.json
```

All 64 sources join, advertise 8 MiB frames, and only admitted sources supply
2 MiB of body. Refusals must use `1013`; the independent session verifies 1 KiB
self echoes. The probe drops admitted incomplete transports intentionally, then
verifies new admission/routing, two normal Close handshakes and final resource
release. Its memory samples are server-only and explicitly phase-bracketed.

To check complete fragmented messages across repeated allocation/reclamation
waves while a separate healthy session continues routing:

```sh
cargo build --release --bins --example fragment_waves
python3 scripts/fragment-waves.py --max-message-bytes 16777216 \
  --output artifacts/fragment-waves.json
```

Eight sources each self-route a 4 MiB CBOR message in 3 MiB and 1 MiB fragments,
for three waves under the usual 16 MiB message ceiling. Interleaved Ping/Pong
proves the first fragments were assembled before completion; returned encoded
values and independent 1 KiB echoes are verified byte for byte. Each wave waits
for cache retirement, then verifies a fresh join/route and normal Close. All
surviving sockets complete normal Close and release their reservations. The
probe uses a one-second reclamation interval and 256 MiB staging allowance to
keep runs short; production defaults remain five seconds and 1 GiB. Use
`--clients 64 --message-bytes 1048576` for the larger active cohort, or
`--reclaim-secs 0 --cooldown-secs 0` to observe retained decoder capacity.

To check unrelated routing while valid messages contain many empty continuation
frames:

```sh
python3 scripts/fragment-fairness.py --workers 1 \
  --output artifacts/fragment-fairness.json
```

Four sources complete eight messages each; every message contains 500,000 empty
continuations. A separate session verifies 1 KiB self echoes every millisecond,
and management latency is sampled alongside server CPU and RSS. Every encoded
value and normal Close handshake is checked. The relay yields after 256 parsed
frames without a completed message, retaining the same incomplete data and
ownership. This provides scheduling opportunities, without a fixed latency
guarantee. Optional `--server-cpus 0 --client-cpus 4-7` separates the client on a
Linux host with those CPUs; `--read-buffer-bytes 131072` checks larger read chunks.

To exercise valid and rejected CBOR forms alongside independent healthy traffic:

```sh
python3 scripts/input-corpus.py --output artifacts/input-corpus.json
```

The independent raw client checks 2,311 exchanges covering 776 input/context
cases. It combines whole messages, mixed-size fragments with interleaved Ping
and 257 empty continuations, and pipelined join/data. Rejected values must never
reach a witness or mutate its channel-variable sentinel; accepted encoded values
must arrive unchanged. It checks exact byte/message/delivery totals, disconnect
notifications, rejection counters and resource release. Complete-message policy
rejections verify the closing handshake; terminal decoder failures verify the
best-effort Close response. Use `--clients 8 --workers 4` for eight concurrent
producers or `--clients 1 --workers 1` for the sequential baseline. The server
remains the executable selected by `--server`; the probe starts a fresh local
process with bounded test settings and a fixture-only management token.

The load tool verifies every frame and its order. Reports include setup time,
verified deliveries, throughput, and end-to-end p50/p99/max latency. Comparisons use
release builds, fresh server processes, sequential candidates, and alternating
order. [docs/performance.md](docs/performance.md) records results and limits;
[docs/STATUS.md](docs/STATUS.md) tracks continuing work.

The stability runner checks sustained traffic, reconnect churn, and an unread
recipient while verifying healthy delivery and sampling memory/management latency.
Current churn measurements include connect, join, a distinct worker/counter
self-echo, the exact Close acknowledgement and TCP EOF. Historical stress
clients used a shorter latency interval and weaker Close checks; the
[validation correction](docs/stability.md#churn-validation-correction-2026-10-16)
records that limit. To exercise address-entry creation and retirement locally:

```sh
python3 scripts/stability.py --modes churn --clients 32 --churn-secs 8 \
  --source-addresses 4096 --max-connections-per-ip 64 \
  --output artifacts/address-churn.json
```

Source binding requires an IPv4 loopback `ws://` literal and a host supporting
the selected `127.64.x.y` source addresses. Zero source addresses retains ordinary
unbound connections. Keep source pool and client build identical when comparing
admission settings.

Its `fanout` mode adds mixed sizes, concurrent healthy readers, optional unread
recipients, a delayed reader, and simultaneous graceful departure:

```sh
python3 scripts/stability.py --modes fanout --clients 128 --fanout-secs 30 \
  --window 8 --payload-sizes 8,1024,524288,65536,256 \
  --unread-clients 1 --reader-delay-ms 5 --output artifacts/mixed-fanout.json
```

Its traffic templates and latency histogram remain bounded as duration increases.
The finite load tool refuses more than 512 MiB of prepared work or 10 million
latency samples; use the stress tool for longer runs. See the measured
[close and backpressure investigation](docs/stability.md).

For a measured low-latency deployment profile, see
[native worker CPU placement](docs/operations.md#native-worker-cpu-placement)
and the [100,000-user latency comparison](docs/performance.md#latency-first-worker-placement-2026-10-17).
