# WebSocket Application Relay

A Rust WebSocket relay with a native CBOR JavaScript client. Clients join a
channel, receive a 64-bit session ID, and send typed values to another session or
their channel. Applications own message meaning, game logic and application state.

The design targets large connected populations with consistently low latency,
shared immutable payloads, per-recipient FIFO and bounded retained memory. A
100,000-user operating profile and short reproducible diagnostics are included;
capacity and latency depend on the workload and host.

## Build and run

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

Open `http://127.0.0.1:4181`, enter the relay URL and run the check. Applications
can import `Relay` from the generated `relay.js` ES module.

## Architecture

- [Native I/O](src/server.rs): fixed Mio readiness workers own decoding and
  timers. Idle sockets need no task, message channel or private receive slab.
  Tokio handles management and process signals.
- [Routing](src/relay.rs): a registry read/write lock protects membership and
  ID ownership. Large native room, global and observer publications retain
  immutable recipient views and advance in bounded turns outside the registry
  lock. Observer revocation finds pending publications through an ordered index.
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
[historical findings](docs/performance-history.md) and
[stability checks](docs/stability.md) describe the contracts and measurement limits.
The [consolidation record](docs/consolidation.md) lists this migration's local
acceptance checks and the next performance milestone.
The [roadmap](docs/roadmap.md) defines the Rust/CBOR product boundary and the
next latency and operational acceptance work.

Disconnect-listener revocation submits only the affected observer's pending
notices before its acknowledgment. Other recipients retain their bounded turns;
a sequence watermark prevents replay after re-subscription. The
[observer-barrier report](docs/observer-barriers.md) describes the measured benefit
and remaining queue-pressure and pending-view scan costs.
Pending disconnect jobs also [share charged observer snapshots](docs/retirement-snapshots.md),
avoiding repeated list copies and repeated reservations for the same storage.
An observer barrier reuses its lookup across jobs sharing a captured view.
When a departure cannot fund deferred work, its synchronous fallback finishes
only [preceding jobs for overlapping channels](docs/retirement-fallback.md).
Unrelated rooms retain their bounded turns, including under source-local pressure.
Native workers give deferred departures [shorter 128-recipient turns](docs/retirement-turns.md)
to reduce Close/control stalls while keeping live fanout at 1,024 recipients.
Native departures above that 128-recipient budget normally defer at publication.
Global broadcasts now use the same bounded live-fanout continuation; the
[global broadcast report](docs/global-broadcasts.md) records short 100,000-reader
checks and the remaining cold-view and observer-path limits.
The [observer-message baseline](docs/observer-messages.md) records the former
synchronous publication stalls. [Bounded observer publications](docs/observer-publications.md)
preserve captured eligibility and acknowledgment ordering while yielding within a message.

## Provenance

Derived from [Nowayz/WebSocket-Application-Relay](https://github.com/Nowayz/WebSocket-Application-Relay)
at upstream commit `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`. The original C++
implementation and later comparison candidates are preserved in Git history,
along with their complete benchmark records. The maintained implementation is
Rust/CBOR. [MIT license](LICENSE). Vendored Rust codec sources retain their
licenses and [recorded patch provenance](vendor/README.md).
