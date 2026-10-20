# Native CBOR client

`@war/relay-client` is an ES module for browsers and Node 22+. This repository
currently provides the package locally; it is not published by the build.
`relay.mjs` and `relay.d.ts` are the implementation and TypeScript entry points.

Install from the repository with `npm ci --prefix clients/js --omit=optional
--ignore-scripts`. `npm run build:browser --prefix clients/js` writes a standalone
ES module, declarations and an interactive check into `target/browser/`.
The explicit JavaScript codec entries work without optional native extractors;
the bundler runs as WebAssembly.

```js
import { Relay, BROADCAST, LISTEN } from './relay.js'; // browser bundle

const client = await Relay.connect('wss://relay.example', 'room');
client.addEventListener('message', event => {
  const { sender, value } = event.detail; // sender is always bigint
  console.log(sender, value);
});
client.send(BROADCAST, { position: [1.5, 2], bytes: new Uint8Array([1, 2]) });
client.send(client.userId, 'self echo');
client.close();
```

Channels are strings or `Uint8Array` values with at most 16 encoded bytes. An
empty channel is valid. A connection stays in its initial channel. Joining
captures the supplied channel before socket setup so later caller mutations do
not change the requested channel.

`send(target, value)` accepts `bigint` IDs and safe integer numbers. Use `bigint`
for the full uint64 range; rounded numbers cannot recover their original ID.
`broadcast(value)` excludes the sender and reaches its room plus eligible global
observers. Payloads can contain strings, bytes, booleans, arrays, maps/objects,
numbers, bigint integers and null, subject to the server's bounded CBOR rules.
Avoid codec-specific extensions and values outside that contract.

Large files can be sent as one value. For a browser `File`, use
`client.send(peerId, new Uint8Array(await file.arrayBuffer()))`; no Base64 or
application-level chunking is required for a 5 MB image. The default server and
both deployment profiles allow 16 MiB per encoded message, including the routing
envelope, with a 32 MiB per-session output ceiling. These ceilings are not
preallocated. Shared global budgets still bound concurrent transfers; the
application must pace traffic to the available network and queue capacity.

Commands return promises and serialize in invocation order because the wire
protocol has no request IDs:

| Method | Result / purpose |
| --- | --- |
| `authenticate(password)` | Boolean authentication status; password is string or bytes |
| `listen(flags)` | Effective low three bits; requires authentication |
| `channels()` | Sorted `[Uint8Array name, population]` pairs; requires authentication |
| `assignId(id)` | Acknowledged `bigint`; requires authentication |
| `setVariable(key, value)` | Acknowledged boolean success |
| `getVariable(key)` | `{ found, value }`; a missing entry differs from a stored null |

Use `LISTEN.CHANNEL`, `LISTEN.PRIVATE` and `LISTEN.DISCONNECT` on an authenticated
`re_globl` connection. These are administrative observation privileges, not a
client authorization mechanism. Variable keys are strings or bytes of 1–255
encoded bytes. Command values are captured when invoked, before asynchronous
dispatch permits callers to mutate their objects.

`message` events carry `{ sender, value }`, `disconnect` events carry
`{ userId }` for a departed peer, `close` events carry `{ code, reason }`, and
`error` events carry the decoding/protocol error. `ready` reports the socket's
open state. `ws` exposes the underlying WebSocket for lifecycle observation.
`Relay.connect(url, channel, { requestTimeoutMs, WebSocketClass })` permits a
custom WebSocket constructor and timeout. The default request/join timeout is ten
seconds. A command timeout closes the connection so its late reply cannot be
mistaken for another command. The client does not silently reconnect or replay
messages: applications decide whether those actions are safe.

In TypeScript, message values default to `unknown`. Narrow them in application
code. `getVariable<MyState>(key)` provides a discriminated result: after checking
`found`, the value is typed as `MyState`; a missing result has `value: null`.

Run `bash scripts/check-js.sh` from the repository root for a fresh native server,
real WebSocket integration and resource-cleanup checks. Run `npm run check:types
--prefix clients/js` for the public declaration contract. The browser page checks
the generated deliverable rather than a separate source-only example.
