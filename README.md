# WebSocket Application Relay — Rust / CBOR

A small, bounded relay for applications and multiplayer games. Clients join a
channel, receive a 64-bit session ID, and send CBOR values to another ID or their
channel. Applications own message meaning and state.

This is a behavioral port of [Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay)
at commit `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`, with the requested CBOR redesign.
The original C++ source, JavaScript clients, dependencies, history, and MIT license
remain available. The original README is in [docs/upstream-README.md](docs/upstream-README.md).

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

Unused enlarged receive storage is released after five seconds without large
input, while clients stay connected. `--read-buffer-idle-secs 0` disables that
policy. It preserves published payload ownership and partial input; the narrow
codec hook and source provenance are documented in [vendor/README.md](vendor/README.md).

Operators can optionally retire inactive joined sessions with
`--idle-timeout-secs 300` (`WAR_IDLE_TIMEOUT_SECS=300`). Binary application input
and outgoing relay messages refresh activity; receive-only consumers stay active
while messages arrive. Ping/Pong traffic does not refresh this deadline. Expiry
closes with `1001` / `Application Idle` and uses normal queue/variable cleanup.
The default `0` disables this policy. See [operations.md](docs/operations.md) for
memory budgeting and deadline details.

## JavaScript client

Install with `npm ci --prefix clients/js --ignore-scripts`. The client supports
modern browsers through an ES module bundler and Node 22+. IDs are `BigInt`.

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

`authenticate(password)`, `listen(flags)`, `channels()`, and `assignId(bigint)`
expose privileged commands. `disconnect` events carry `{ userId }`; `close` events
carry `{ code, reason }`. The client serializes control requests, including
concurrent variable requests, because v1 deliberately has no request-ID field.

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

Forwarding keeps the upstream's zero-copy intent: a validated application value
stays in its received buffer, shared across recipient queues. The server builds
only the small replacement sender header and writes headers plus the borrowed
body with vectored I/O. Changing a CBOR ID's encoded width does not shift the
body. Tiny slices of much larger receive storage are compacted deliberately to
avoid pinning that storage in a slow queue. This concerns user-space forwarding;
WebSocket unmasking, fragmented-message assembly, and kernel transfers still do
their required work. See [docs/performance.md](docs/performance.md).

Native decoder staging has a shared 1 GiB allowance, configurable with
`--total-read-buffer-bytes` / `WAR_TOTAL_READ_BUFFER_BYTES` (`0` disables).
Frame and fragment storage reserve twice the requested capacity before growth,
retaining headroom for reuse. Exhaustion returns HTTP `503` before upgrade or a
best-effort WebSocket `1013` before reading a refused body. This preserves shared
payload ownership and avoids per-packet budget atomics within the high water.
It is a staging allowance, not an RSS limit; see [operations](docs/operations.md)
for the accounting boundary, reclamation and metrics.

## Original clients

The optional migration bridge translates the original binary/text wire format:

```sh
./target/release/war-legacy-bridge \
  --bind 127.0.0.1:1337 --upstream ws://127.0.0.1:1338
```

Existing `relay.js` clients connect to `ws://127.0.0.1:1337`. Their binary/text
payloads become CBOR byte/text strings and interoperate with native CBOR clients.
Other structured CBOR values require a CBOR client: an incompatible legacy
recipient closes with `1003`, rather than receiving an invented encoding.

The bridge preserves original command/reply layouts and suppresses CBOR command
acknowledgements. Tests execute the **unchanged upstream `relay.js`** on real
sockets. The old `relay.module.js` remains as source history; its existing
undefined-variable and const-reassignment bugs are not copied into the new client.
An isolated check also runs the **unchanged original C++ executable** and compares
46 shared wire behaviors against the bridge, while asserting eight deliberate
behavior differences. See [compatibility.md](docs/compatibility.md) for the
recorded results and local dependency setup.

## Operating the relay

- Connection admission includes TCP handshakes, with handshake and join deadlines.
  Channel populations and inbound message/byte rates have explicit limits.
- Each recipient has a bounded message queue and byte budget, including an
  in-flight write. Routing never awaits a consumer; overflow closes it with `1013`.
  Pending writes observe session cancellation and release their reservations promptly.
- Process-wide queue and variable byte budgets bound aggregate retained payloads
  as the number of users and channels grows. Read buffers default to 4 KiB.
- Channel variables have count and byte limits and disappear with an empty
  ordinary channel. `re_globl` persists, matching the original behavior.
- OS entropy supplies IDs. Fixed-ID takeover closes the old owner with `4001`;
  old-session cleanup cannot erase its replacement. Reserved IDs cannot be claimed.
- Tokio drives concurrent connections. Routing uses shared registry reads;
  membership/ID changes use writes. No registry lock spans an await or socket I/O.
  Fanout shares one immutable application body across recipients.
- SIGINT/SIGTERM stop admission and enforce a shutdown deadline. Responsive peers
  receive `1001`; blocked transports can be dropped. Peer-initiated Close frames
  receive their exact acknowledgement. Cleanup also runs on task cancellation.
  A cancelled write that has not started preserves the normal close handshake;
  cancellation of a possibly partial frame still discards its transport.

Management uses a separate HTTP listener:

| Endpoint | Purpose | Authorization |
| --- | --- | --- |
| `GET /healthz` | Liveness | None |
| `GET /readyz` | `503` while draining | None |
| `GET /metrics` | Prometheus counters/gauges | Bearer token when configured |
| `GET /v1/channels` | Populations and variable usage | Bearer token required |
| `POST /v1/drain` | Reject new connections/joins | Bearer token required |
| `POST /v1/resume` | Resume admission | Bearer token required |
| `POST /v1/sessions/{decimal_id}/disconnect` | Close session with `4000` | Bearer token required |

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
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
./scripts/check-js.sh
npm run build:browser --prefix clients/js
```

The codec source audit uses Python 3.12+ and Git, caching two checksum-pinned
published archives on its first run. Rust builds use the included codec sources.

The JavaScript script builds temporary loopback services, runs the CBOR and
original-client checks with idle expiry disabled, then restarts the relay to
verify idle Close propagation through native and legacy WebSockets. It removes
the temporary services afterward.
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

The load tool verifies every frame and its order. Reports include setup time,
verified deliveries, throughput, and end-to-end p50/p99/max latency. Comparisons use
release builds, fresh server processes, sequential candidates, and alternating
order. [docs/performance.md](docs/performance.md) records results and limits;
[docs/STATUS.md](docs/STATUS.md) tracks continuing work.

The stability runner checks sustained traffic, reconnect churn, and an unread
recipient while verifying healthy delivery and sampling memory/management latency.
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
