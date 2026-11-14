# WebTransport Application Relay

A Rust WebTransport relay with a native CBOR JavaScript client. Clients join a
channel, receive a 64-bit session ID, and send typed values to another session or
their channel. Applications own message meaning, game logic and application state.

The design targets large connected populations with consistently low latency,
shared immutable payloads, per-recipient FIFO and bounded retained memory. Each
session is one QUIC connection: reliable messages travel in order on one
bidirectional stream, and unreliable state updates can use datagrams. A
100,000-session operating profile and short reproducible diagnostics are
included; capacity and latency depend on the workload and host.

## Build and run

For release assets, installation and the final acceptance gate, see the
[release guide](docs/release.md).

Rust 1.91+ builds the server; its bundled jemalloc allocator also needs a C
compiler and `make`. Node 22+ is needed for the JavaScript tools.

```sh
cargo build --locked --release --bin war-relay
./target/release/war-relay --dev-cert
```

The data endpoint is `https://127.0.0.1:1338/war` (HTTP/3 over UDP) and local
management is `http://127.0.0.1:1339`. `--dev-cert` generates a temporary
localhost certificate and logs its SHA-256 pin, which local clients pass as
`serverCertificateHashes`. Production uses a publicly trusted certificate with
`--tls-cert` and `--tls-key`; see [deploy/README.md](deploy/README.md). Run
`war-relay --help` for configuration and environment variables.
`WAR_AUTH_PASSWORD` enables privileged relay commands; `WAR_ADMIN_TOKEN` enables
authenticated management operations. Neither grants privileges when absent.
Secrets are environment-only and diagnostics omit values.

## JavaScript and TypeScript

The client is [clients/js/relay.mjs](clients/js/relay.mjs). It uses standard
CBOR, keeps received IDs as `bigint`, runs wherever `WebTransport` is available
(current browsers), and includes native TypeScript declarations. The browser
build uses a WebAssembly bundler and requires no native addon installation or
install scripts.

```sh
npm ci --prefix clients/js --omit=optional --ignore-scripts
npm run check:types --prefix clients/js
npm run build:browser --prefix clients/js
```

```js
import { Relay } from './relay.js'; // browser bundle

const relay = await Relay.connect('https://relay.example/war', 'lobby');
relay.addEventListener('message', ({ detail: { sender, value } }) => {
  console.log(sender, value);
});
relay.broadcast({ text: 'hello π', bytes: new Uint8Array([0, 255]), turn: 42n });
relay.send(relay.userId, ['self', true, null]);
relay.broadcastUnreliable({ position: [1.5, 2] }); // one datagram, may be lost
await relay.setVariable('state', { round: 1 });
console.log(await relay.getVariable('state')); // { found: true, value: { round: 1 } }
relay.close();
```

Use `bigint` for IDs outside JavaScript's safe integer range. Detailed client
usage is in [clients/js/README.md](clients/js/README.md).

The browser bundle and interactive check are written to `target/browser/`:

```sh
python3 -m http.server 4181 --bind 127.0.0.1 --directory target/browser
```

Open `http://127.0.0.1:4181`, enter the relay URL and certificate pin, and run
the check. It includes 128 KiB and 5 MiB binary values; both supplied deployment
profiles support these. Applications can import `Relay` from the generated
`relay.js` ES module.

## Architecture

The whole relay is one file, [src/lib.rs](src/lib.rs); each part below is a
module in it. [src/main.rs](src/main.rs) is the command-line entry point and
[src/tests/](src/tests) holds the unit tests.

- WebTransport (`webtransport`): one shared UDP socket, serviced by a
  dedicated ingress thread, feeds QUIC connections driven on a multi-threaded
  Tokio runtime whose workers are pinned one per CPU when the process owns
  exactly that many. Each session reads length-prefixed CBOR records from its
  bidirectional stream and datagrams from the connection. The QUIC and TLS
  stack is vendored with storage, scheduling and loss-recovery patches; see
  [vendor/README.md](vendor/README.md).
- Routing (`relay`): no global registry lock. Session and channel indexes
  are concurrent maps; each channel locks its own membership, variables and
  cached recipient view. Global observers use atomic immutable snapshots, and
  admission counters are atomics. No stream write happens while a registry lock
  is held. Large broadcasts capture an immutable recipient view and advance in
  bounded turns; ordinary rooms have their own departure queues and observer
  notices use sixteen shards. See [docs/architecture.md](docs/architecture.md).
- Output (`webtransport::queue`): private sends attempt an immediate stream
  write; otherwise each session's output queue keeps FIFO order, partial writes
  and write deadlines.
- Frames (`frame`): inline routing prefixes and shared `Bytes` bodies avoid
  rebuilding the application payload for each destination.
- Management (`admin`): health/readiness, metrics, paginated channel
  inspection, administrative disconnect and reversible drain/resume.

Admission, record staging, retained output and channel variables have explicit
resource budgets. Slow recipients cannot grow unbounded application queues.
Terminal shutdown suppresses departure traffic and releases listener indexes
before bulk session retirement. Applications provide their own authorization
model; channel names and session IDs are routing identifiers, not secrets.

## Verify and measure

```sh
cargo fmt --check
python3 scripts/check-vendor.py
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
bash scripts/check-js.sh
npm run build:browser --prefix clients/js
```

[Protocol](docs/protocol.md), [operations](docs/operations.md),
[short performance recipes](docs/performance-guide.md),
[measured performance](docs/performance.md) and
[verified open-loop transport loads](docs/datagram-load.md) describe the
contracts and measurement limits. The [roadmap](docs/roadmap.md) defines the
product boundary and the next work.

## Delivery guarantees

Everything one session sends reliably reaches each recipient in order, followed
by that session's departure notice. A broadcast reaches every recipient captured
when it started, even if the sender leaves before delivery finishes. Under
output pressure delivery is best-effort: a slow recipient is closed with `1013`,
and when the global budget is exhausted the message is dropped for that
recipient while the sender is never closed. Datagrams are unreliable and
unordered. Departure notices are ordered within a room, not across rooms. The
full list, with the exact drop policy, is in
[docs/protocol.md](docs/protocol.md#delivery-guarantees); the design is in
[docs/architecture.md](docs/architecture.md).

## Provenance

This repository is [Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay);
the name predates the move to WebTransport. Earlier transports remain in Git
history. [MIT license](LICENSE). Vendored Rust sources retain their licenses
and [recorded patch provenance](vendor/README.md).
