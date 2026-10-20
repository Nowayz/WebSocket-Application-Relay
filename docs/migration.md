# Migration to the Rust/CBOR product

The sole maintained server is `war-relay`; the sole JavaScript client is
`clients/js/relay.mjs` (or its generated browser bundle). The original Binary/Text
wire formats, original JavaScript classes, compatibility bridge, C++ servers and
comparison build systems have been removed. There is no compatibility listener.

Update clients and servers together. Existing Binary/Text clients cannot speak
the CBOR protocol by changing their URL alone. Replace old root-script imports
with the native client; connect with `Relay.connect(url, channel)`, send values
with `send(id, value)` or `broadcast(value)`, and handle `message` event details.
Replace fixed-width little-endian ID decoding with the client's `bigint` IDs.
Replace callback commands with promises. Variable reads return `{ found, value }`
so missing entries and stored null values remain distinguishable.

This retirement concerns wire formats. Strings and binary application data remain
supported as CBOR text strings and byte strings. Both travel inside binary
WebSocket messages, along with structured values. WebSocket text frames are
rejected by the native endpoint.

## Old to new client API

The old `relay.js` exposed one `Relay` class with callbacks and byte-array IDs.
The new client is an `EventTarget` with promises and `bigint` IDs. The table
maps each old member to its replacement.

| Old `relay.js` | New `clients/js/relay.mjs` |
| --- | --- |
| `new Relay(url, channelName)` then `onSubscription` callback | `const relay = await Relay.connect(url, channel)` |
| `relay.userId` (8-byte `Uint8Array`) | `relay.userId` (`bigint`) |
| `relay.ready`, `relay.ws`, `relay.channelName` | `relay.ready`, `relay.ws`; keep the channel in your own code |
| `SetMessageHandler(re.BINARY, opcode, (sender, bytes) => ...)` | `addEventListener('message', e => { const { sender, value } = e.detail; })`; put the opcode inside the value |
| `SetMessageHandler(re.TEXT, opcode, (sender, obj) => ...)` | Same `message` event; `value` is the decoded object |
| `bSendTo(target, opcode, bytes)` | `send(target, value)` with the bytes as a `Uint8Array` inside the value |
| `tSendTo(target, obj)` | `send(target, obj)` |
| `bSendTo(re.BINARY_BROADCAST, ...)`, `tSendTo(re.TEXT_BROADCAST, ...)` | `broadcast(value)` or `send(BROADCAST, value)` |
| `onClientDisconnect = userId => ...` | `addEventListener('disconnect', e => e.detail.userId)` |
| `bSendTo(re.RELAY_QUERY, 0, password)` (no reply) | `await authenticate(password)` returns a boolean |
| `bSendTo(re.RELAY_QUERY, 1, [flags])` (no reply) | `await listen(flags)` returns the effective flags; `LISTEN.CHANNEL`, `LISTEN.PRIVATE`, `LISTEN.DISCONNECT` keep the bit values 1, 2 and 4 |
| `bSendTo(re.RELAY_QUERY, 2)` with a packed binary reply | `await channels()` returns `[Uint8Array name, population]` pairs |
| `bSendTo(re.RELAY_QUERY, 3, idBytes)` (no reply) | `await assignId(id)` returns the acknowledged `bigint` |
| `SetChannelVar(key, value)` (no reply) | `await setVariable(key, value)` |
| `GetChannelVar(key, callback)` with reply opcode `200` | `await getVariable(key)` returns `{ found, value }` |
| `re.UInt8UserIdToBase64`, `re.Base64ToUInt8UserID`, `re.UserIdHex` | Not needed; use `id.toString()` or `id.toString(16)` |

Every new command has exactly one reply and commands are serialized in call
order, because the wire protocol has no request IDs. A command that times out
closes the connection so a late reply cannot be matched to another request.

## Old to new wire layout

| Message | Old Binary/Text format | New CBOR format |
| --- | --- | --- |
| Join | Channel name as the first message, at most 16 bytes | CBOR text or byte string, 0 to 16 bytes, in a binary frame |
| Welcome | 8 raw little-endian bytes | CBOR `uint64` |
| Binary send | `[8-byte LE target][1-byte opcode][payload]` | `[target: uint64, value]`, one CBOR item |
| Text send | `[12-char base64 target][JSON with "op"]` | Same `[target, value]`; JSON objects become CBOR maps |
| Forwarded message | `[8-byte LE sender][opcode][payload]` or `[12-char base64 sender][JSON]` | `[sender: uint64, value]` |
| Broadcast target | `0xFFFFFFFFFFFFFFFF` as 8 bytes or `//////////8=` | `18446744073709551615` |
| Relay command target | 8 zero bytes | `0`, with `[0, [opcode, argument]]` |
| Command reply | Only opcodes 2 and 5 replied, in packed binary layouts | `[0, [reply_opcode, result]]` for every command |
| Disconnect event | 16 bytes: 8 bytes of `0xFF` then the 8-byte departed ID | `[18446744073709551615, departed_id]` |
| Channel variable value | Raw bytes after opcode `200` | Any CBOR value; reads return `[found, value]` |

Opcodes 0 to 5 keep their meanings: authenticate, listener flags, channel list,
assign ID, set variable, get variable. Close codes `1002`, `1003`, `1013` and
`4001` keep their meanings; `1013` has a narrower definition, listed in
[protocol.md](protocol.md). The full new protocol is in that page.

## Cutover procedure

The old and new servers cannot share a listener, and old clients cannot talk
to the new server. Run both during the switch:

1. Deploy `war-relay` on a new port (or a new ingress route) next to the
   existing server. Set `WAR_AUTH_PASSWORD`, `WAR_ADMIN_TOKEN`,
   `WAR_ALLOWED_ORIGINS` and the resource budgets for the host; see
   [operations.md](operations.md). Verify it with the browser check or
   `bash scripts/check-js.sh`.
2. Migrate clients to `clients/js/relay.mjs` (or the browser bundle) and point
   them at the new address. Clients on both servers cannot see each other, so
   move all participants of one application at once.
3. When no client connects to the old server, drain it, stop it and remove its
   service and ingress route. Keep its binary and client for rollback until the
   new deployment has run through a normal load cycle.

Use the native relay and management addresses, ordinarily ports 1338 and 1339.
Set independent relay and management secrets, configure browser Origins and
terminate TLS at your ingress. The repository's systemd and Caddy examples
describe the native service.

## Where the old implementation lives

The project retains its MIT license and upstream attribution. Complete previous
implementations and evidence exist in Git history at `587c1cd`. These commands
need a full clone; a shallow clone does not have that commit:

```sh
git show 587c1cd:relay.js
git show 587c1cd:relay.cpp
git show 587c1cd:cpp/BENCHMARKS.md
git show 587c1cd:docs/benchmarks/2026-10-19-global-shutdown-indexes/README.md
```

The concise [performance history](reports/performance-history.md) retains the
useful findings without keeping obsolete binaries, source snapshots, charts and
raw benchmark dumps in the maintained tree. Future comparisons use previous Rust
revisions with identical native CBOR clients and workloads.
