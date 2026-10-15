# WAR CBOR v1

## Framing

One CBOR item per binary WebSocket message. WebSocket fragmentation is reassembled
by tungstenite; the relay processes complete messages. Text WebSocket messages
close with `1003`. The first CBOR item is a definite-length text or byte string
containing a channel name, 0–16 decoded bytes. Text uses strict UTF-8. The reply is
a CBOR uint64, excluding 0 and `18446744073709551615`.

Subsequent messages are `[target: uint64, value: CBOR]`. Forwarded messages are
`[sender: uint64, value: CBOR]`. CBOR integer byte order is independent of the
machine; the original little-endian encoding exists only in the legacy bridge.
The sender prefix is stored separately from the received application value and
gathered into one ordinary binary WebSocket message at write time. Both compact
and full-width target IDs remain accepted; no fixed-width CBOR requirement or
additional WebSocket fragmentation is introduced by routing.

Values support all definite-length CBOR major types, tags, half/single/double
floats, null, undefined, and simple values. Non-minimal integer encodings are
accepted. Duplicate map keys and tag meanings are application responsibilities:
the relay preserves opaque encoded values. Indefinite containers, malformed UTF-8,
trailing items, truncated arguments, more than 64 nesting levels, or more than
65,536 items per value are protocol violations. Validation uses a fixed stack and
does not allocate an application object graph.

Expanded simple values `f8 00` through `f8 1f` are malformed under
[RFC 8949 section 3.3](https://www.rfc-editor.org/rfc/rfc8949.html#section-3.3)
and close the producing session with `1002`, including when nested or supplied
as a variable. One-byte simple values and expanded values `f8 20` through
`f8 ff` remain accepted and opaque. This rule differs from nonminimal integer
encodings, which remain accepted and unchanged.

## Routing

| Target / sender | Meaning |
| --- | --- |
| Target `0` | Relay command |
| Target `18446744073709551615` | Broadcast, excluding sender |
| Other target | Private destination, across channels, including self |
| Sender `0` | Command reply |
| Sender `18446744073709551615` | Disconnect event; value is departed uint64 ID |

Unknown destinations are dropped. A connection's messages retain order;
concurrent senders have no global total-order guarantee. A new connection's first
received item is its ID. A socket remains tied to its channel until disconnection.

`re_globl` broadcasts reach all sessions, while privileged listeners in that
channel can receive selected traffic. `--disable-global-broadcast` requires
authentication for global sends. This does not change listener authorization.

## Commands

Send `[0, [opcode: uint8, argument]]`; receive `[0, [reply_opcode, result]]`. Every
authorized well-formed command has one reply. Unauthenticated privileged commands
close with `1008`. Failed authentication returns false; an already authenticated
connection stays authenticated after another attempt.

| Opcode | Argument | Result | Authentication |
| --- | --- | --- | --- |
| `0` authenticate | Text/byte password | Boolean authenticated state | No |
| `1` listener flags | uint8: channel=1, private=2, disconnect=4 | Effective low 3 bits | Yes |
| `2` channels | `null` | Array of `[channel_bytes, population]`, sorted by raw name | Yes |
| `3` assign own ID | Non-reserved uint64 | Assigned uint64 | Yes |
| `4` set channel variable | `[key_text_or_bytes, value]` | `true` | No, matching upstream |
| `5` get channel variable | Key text/bytes | Reply opcode `200`: `[found: bool, value]` | No, matching upstream |

Variable keys contain 1–255 decoded bytes; text/byte keys with identical bytes
address the same entry. Values retain their exact CBOR bytes. Missing entries
return `[false, null]`, while a stored null returns `[true, null]`. Byte budgets
count key bytes plus encoded value bytes. Ordinary-channel values disappear with
the last member; global variables persist until restart.

ID reassignment is atomic with routing. It closes a displaced owner with `4001`.
Assigning one's current ID succeeds. A displaced owner's disconnect event reports
its actual departed ID, fixing upstream's occasional zero-ID event after takeover.
Global-channel disconnect notifications are delivered once instead of duplicating
the same event through both membership and listener paths.

## Independent known vectors

| Meaning | CBOR diagnostic form | Hex bytes |
| --- | --- | --- |
| Join | `"hi"` | `62 68 69` |
| Welcome ID 42 | `42` | `18 2a` |
| Send to 42 | `[42, "hi"]` | `82 18 2a 62 68 69` |
| Forward from 1 | `[1, "hi"]` | `82 01 62 68 69` |
| Broadcast bytes | `[18446744073709551615, h'0701']` | `82 1b ff ff ff ff ff ff ff ff 42 07 01` |
| Query channels | `[0, [2, null]]` | `82 00 82 02 f6` |
| Read `x` | `[0, [5, "x"]]` | `82 00 82 05 61 78` |
| Missing variable | `[0, [200, [false, null]]]` | `82 00 82 18 c8 82 f4 f6` |
| Departed ID 42 | `[18446744073709551615, 42]` | `82 1b ff ff ff ff ff ff ff ff 18 2a` |

See [RFC 8949](https://www.rfc-editor.org/rfc/rfc8949.html) for CBOR encoding rules.

## Close codes

| Code | Cause |
| --- | --- |
| `1000` | Normal close |
| `1001` | Shutdown or heartbeat timeout |
| `1002` | Invalid CBOR/WAR or WebSocket framing |
| `1003` | Text at CBOR endpoint, or structured CBOR at a legacy recipient |
| `1008` | Authentication, rate, join-timeout, reserved-ID, or global-send policy |
| `1009` | Incoming message exceeds configured size |
| `1013` | Admission/variable capacity or recipient queue budget |
| `4000` | Operator disconnect |
| `4001` | Privileged fixed-ID takeover |

Admission refusals before upgrade can end with a TCP close instead of a WebSocket
close frame. The bridge suppresses modern command acknowledgements to preserve
original-client expectations. See the README for its supported value conversions.

Close codes describe the intended policy. If a transport is blocked partway
through a frame, cancellation drops it instead of queueing another write behind
the blocked data. Such a peer may observe an abnormal close/reset; `1006` is a
client-side report and is never transmitted as a close frame. A cooperative peer's
valid Close code and reason are echoed before dropping its transport, including
connections that close before joining a channel.
