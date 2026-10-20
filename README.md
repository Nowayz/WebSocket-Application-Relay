# WebSocket Application Relay

A Rust WebSocket relay with a native CBOR JavaScript client. Clients join a
channel, receive a 64-bit session ID, and send typed values to another session or
their channel. Applications own message meaning, game logic and application state.

The design targets large connected populations with consistently low latency,
shared immutable payloads, per-recipient FIFO and bounded retained memory. A
100,000-user operating profile and short reproducible diagnostics are included;
capacity and latency depend on the workload and host.

## Build and run

For release assets, installation and the final acceptance gate, see the
[release guide](docs/release.md).

Rust 1.91+ builds the server. Node 22+ is needed for the JavaScript tools.

```sh
cargo build --locked --release --bin war-relay
./target/release/war-relay
```

Defaults are `ws://127.0.0.1:1338` and local management at
`http://127.0.0.1:1339`. Run `war-relay --help` for configuration and environment
variables. `WAR_AUTH_PASSWORD` enables privileged relay commands;
`WAR_ADMIN_TOKEN` enables authenticated management operations. Neither grants
privileges when absent. Secrets are environment-only and diagnostics omit values.

## JavaScript and TypeScript

The sole client is [clients/js/relay.mjs](clients/js/relay.mjs). It uses standard
CBOR, keeps received IDs as `bigint`, supports browsers and Node 22+, and includes
native TypeScript declarations. The client uses the JavaScript codec entry points;
optional native codec addons are omitted. The browser build uses a WebAssembly
bundler and requires no native addon installation or install scripts.

```sh
npm ci --prefix clients/js --omit=optional --ignore-scripts
npm run check:types --prefix clients/js
npm run build:browser --prefix clients/js
```

From Node, run this in the repository root:

```js
import { Relay } from './clients/js/relay.mjs';

const relay = await Relay.connect('ws://127.0.0.1:1338', 'lobby');
relay.addEventListener('message', ({ detail: { sender, value } }) => {
  console.log(sender, value);
});
relay.broadcast({ text: 'hello π', bytes: new Uint8Array([0, 255]), turn: 42n });
relay.send(relay.userId, ['self', true, null]);
await relay.setVariable('state', { round: 1 });
console.log(await relay.getVariable('state')); // { found: true, value: { round: 1 } }
relay.close();
```

Use `bigint` for IDs outside JavaScript's safe integer range. CBOR strings and
byte arrays are ordinary application values carried in binary WebSocket frames.
The old Binary/Text wire formats and their bridge are retired; see the
[migration notes](docs/migration.md). Detailed client usage is in
[clients/js/README.md](clients/js/README.md).

The browser bundle and interactive check are written to `target/browser/`:

```sh
python3 -m http.server 4181 --bind 127.0.0.1 --directory target/browser
```

Open `http://127.0.0.1:4181`, enter the relay URL and run the check. It includes
128 KiB and 5 MiB binary values; both supplied deployment profiles support these.
Applications
can import `Relay` from the generated `relay.js` ES module.

## Architecture

- [Native I/O](src/server.rs): fixed Mio readiness workers own decoding and
  timers. Idle sockets need no task, message channel or private receive slab.
  Fresh readiness and buffered/output work receive separate bounded turns;
  [scheduling measurements](docs/reports/worker-scheduling.md) document the latency/CPU
  tradeoff. Tokio handles management and process signals.
- [Routing](src/relay.rs): no global registry lock. ID ownership is a 64-way
  sharded map, each channel has its own lock for membership, variables and its
  cached recipient view, global observers have a separate lock, and admission
  counters are atomics. No socket write happens while a registry lock is held.
  Large broadcasts capture an immutable recipient view and advance in bounded
  turns; each room has its own departure queue. See
  [docs/architecture.md](docs/architecture.md).
- [Output](src/server/socket.rs): private sends attempt immediate borrowed writes;
  multicast uses recipient-owner output and bounded vectored batches. Each socket
  serializes output so partial writes, commands and control frames retain order.
- [Frames](src/frame.rs): inline routing prefixes and shared `Bytes` bodies avoid
  rebuilding the application payload for each destination. This does not mean
  zero kernel copying or zero first-use allocation.
- [Management](src/admin.rs): health/readiness, metrics, paginated channel
  inspection, administrative disconnect and reversible drain/resume.

Admission, decoder staging, retained output and channel variables have explicit
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
npm run check:types --prefix clients/js
npm run build:browser --prefix clients/js
```

[Protocol](docs/protocol.md), [operations](docs/operations.md),
[short performance recipes](docs/performance-guide.md),
[historical findings](docs/reports/performance-history.md) and
[stability checks](docs/reports/stability.md) describe the contracts and measurement limits.
The [consolidation record](docs/reports/consolidation.md) lists this migration's local
acceptance checks and the next performance milestone.
The [roadmap](docs/roadmap.md) defines the Rust/CBOR product boundary and the
next latency and operational acceptance work.

## Delivery guarantees

Everything one session sends reaches each recipient in order, followed by that
session's departure notice. A broadcast reaches every recipient captured when
it started, even if the sender leaves before delivery finishes. Under output
pressure delivery is best-effort: a slow recipient is closed with `1013`, and
when the global budget is exhausted the frame is dropped for that recipient
while the sender is never closed. Departure notices are ordered within a room,
not across rooms. The full list, with the exact drop policy, is in
[docs/protocol.md](docs/protocol.md#delivery-guarantees); the design is in
[docs/architecture.md](docs/architecture.md) and the dated measurement records
are in [docs/reports/](docs/reports/README.md).

## Provenance

This repository is [Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay).
The Rust implementation replaced the original 2018 C++ server in October 2026.
The original server, its `relay.js` client and the comparison candidates with
their complete benchmark records are preserved in Git history at commit
`587c1cd`; see [docs/migration.md](docs/migration.md) for retrieval commands.
The maintained implementation is Rust/CBOR. [MIT license](LICENSE). Vendored
Rust codec sources retain their licenses and
[recorded patch provenance](vendor/README.md).
