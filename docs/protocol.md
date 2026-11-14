# WAR CBOR v1

## Transport

A session is one WebTransport session (HTTP/3 over QUIC) at the path `/war`, for
example `https://relay.example/war`. Any other path is refused with HTTP `404`.
The session request optionally enforces an exact browser Origin allowlist through
`--allowed-origin` / `WAR_ALLOWED_ORIGINS`. The default remains unrestricted.
With a list, denied or duplicate Origin headers receive HTTP `403`; a missing
Origin remains allowed for native clients. This is separate from authentication
and leaves CBOR routing unchanged. See
[operations](operations.md#browser-origin-admission).

The client opens one bidirectional stream. Each direction carries records: a
4-byte big-endian length followed by that many bytes of one CBOR item. The
first client record is a definite-length text or byte string containing a
channel name, 0–16 decoded bytes. Text uses strict UTF-8. The reply record is a
CBOR uint64, excluding 0 and `18446744073709551615`.

Subsequent records are `[target: uint64, value: CBOR]`. Forwarded records are
`[sender: uint64, value: CBOR]`. CBOR integer byte order is independent of the
machine. The sender prefix is stored separately from the received application
value and gathered into one record at write time. Both compact and full-width
target IDs remain accepted.

A datagram carries exactly one complete `[target, value]` item with no length
prefix. Datagrams are unreliable and unordered; they share the session's
membership, routing and rate limits with its stream. Relay commands (target
`0`) are refused on datagrams: the session closes with `1002`. A datagram
forwarded to a recipient whose negotiated maximum datagram size is smaller is
dropped for that recipient.

An empty record (length zero) from the client is a graceful close: the relay
removes the session from routing, delivers its departure notices, and answers
with an empty record. The client then closes the connection.

Values support all definite-length CBOR major types, tags, half/single/double
floats, null, undefined, and simple values. Non-minimal integer encodings are
accepted. Duplicate map keys and tag meanings are application responsibilities:
the relay preserves opaque encoded values. Indefinite containers, malformed UTF-8,
trailing items, truncated arguments, more than 64 nesting levels, or more than
65,536 items per value are protocol violations. Validation uses a fixed stack and
does not allocate an application object graph. The maximum encoded message,
including the routing envelope, is `WAR_MAX_MESSAGE_BYTES` (16 MiB by default).

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

Unknown destinations are dropped. A session's reliable messages retain order;
concurrent senders have no global total-order guarantee. A new session's first
received record is its ID. A session remains tied to its channel until it ends.

`re_globl` broadcasts reach all sessions, while privileged listeners in that
channel can receive selected traffic. `--disable-global-broadcast` requires
authentication for global sends. This does not change listener authorization.
A broadcast from the only member of a room still reaches authorized
channel-message observers; it does not echo to the sender.

## Delivery guarantees

These hold for every recipient the relay has accepted output for. The design
behind them is in [architecture.md](architecture.md); terms are in the
[glossary](glossary.md).

- **Per-source FIFO.** Everything one session produces reaches each recipient
  in the order the relay processed it: its messages, its command replies and
  finally its departure notice. The welcome is the first item a session
  receives. A large broadcast is finished before the same source's next
  message is routed.
- **A broadcast reaches every captured recipient even if the source leaves.**
  Membership is captured when a broadcast starts. If the sending session
  closes before delivery finishes, the remaining recipients still receive the
  message through the departure queue. In an ordinary room it arrives before
  that session's departure notice. A `re_globl` broadcast that outlives its
  source is completed in a separate queue and is not ordered against the
  source's departure notice.
- **Best-effort under output pressure.** A recipient whose own queue is full
  (`WAR_QUEUE_BYTES` or `WAR_QUEUE_MESSAGES`) is closed with `1013` and
  receives nothing more. When the global budget `WAR_TOTAL_QUEUE_BYTES` is
  exhausted, the frame for the recipient being charged is dropped and
  `war_global_pressure_total` increments; that recipient is closed with `1013`
  only if it already holds at least a quarter of its own `WAR_QUEUE_BYTES`, or
  if the dropped frame was the reply to its own command. The sender is never
  closed because of a recipient. Recipients are visited in order, so later
  recipients of one broadcast may miss a frame that earlier recipients
  received. Already submitted output is never rolled back.
- **Departure notices are ordered within a room, not across rooms.** Within
  one room a departure notice follows the departed session's last messages.
  Notices for different rooms are advanced by independent queues and have no
  order relative to each other. A departed peer completing its graceful close
  does not prove that other recipients received its notice.
- **Datagrams are best-effort.** A datagram may be lost or arrive out of order
  relative to other datagrams and to reliable records. It is never retransmitted.
- **ID takeover.** Assigning an ID held by another session closes that session
  with `4001`. Routing looks up the target atomically with reassignment: a
  message routed after the assignment reply goes to the new owner, while
  output already accepted for the old owner may still finish. The displaced
  session's departure notice reports the ID it actually held.

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

Channel listings (command `2` and HTTP inspection) read each room independently.
During concurrent joins and departures they are not a point-in-time snapshot.
Ordinary rooms with no members, or already marked retired, are omitted at the
metadata read; `re_globl` remains visible even when empty. Stable rooms retain
their exact metadata and the documented response ordering. A concurrent room
may appear or disappear between pages.

For an authenticated global listener, clearing a message-observer bit (`1` or
`2`) submits its previously captured observer messages before the listener-flags
reply. Clearing disconnect bit `4` does the same for captured disconnect-observer
notices. Re-enabling a bit admits future publications without replaying flushed
ones. A publication or departure captured concurrently with the revocation
may be omitted for that observer; it never arrives after the acknowledgement.
Disconnect notices for sessions of different rooms are not ordered relative
to each other; notices for sessions of one room arrive in departure order.
Ordinary room membership still receives its
own room's departure events independently of listener flags. Existing queue
budgets and slow-recipient policy continue to apply. The barrier advances only
that observer's output; unrelated recipients keep their asynchronous turns.
Message revocation may also flush that listener's older pending messages for the
other message class, so one watermark safely covers both classes. Publications
from concurrent senders still have no global total-order guarantee.

Variable keys contain 1–255 decoded bytes; text/byte keys with identical bytes
address the same entry. Values retain their exact CBOR bytes. Missing entries
return `[false, null]`, while a stored null returns `[true, null]`. Byte budgets
count key bytes plus encoded value bytes. Ordinary-channel values disappear with
the last member; global variables persist until restart.

Routing selects the sender ID and target session atomically with ID reassignment.
A route begun after an assignment acknowledgement uses the current owner;
already accepted output may finish later. Reassignment closes a displaced owner with `4001`.
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

Policy closes end the WebTransport session with one of these codes and its
reason (browsers report them through `WebTransport.closed`).

| Code | Cause |
| --- | --- |
| `1000` | Normal close after a graceful close request |
| `1001` | Shutdown or configured application-idle timeout |
| `1002` | Invalid CBOR/WAR, a record longer than `WAR_MAX_MESSAGE_BYTES`, a command sent as a datagram, a join record that does not arrive within `WAR_JOIN_TIMEOUT_SECS`, or any other session failure |
| `1008` | Authentication, rate, reserved-ID, or global-send policy |
| `1013` | Join refused by admission or drain; channel variable capacity; the session's own command reply could not be funded; the session is a slow consumer |
| `4000` | Operator disconnect |
| `4001` | Privileged fixed-ID takeover |

`1013` means exactly one of the following. The join was refused because the
relay is draining or a connection, channel or room limit was reached. A
channel variable write exceeded the per-channel or global variable budget. The
session's own command reply, including its welcome, could not be funded from
its output budget. The session is a slow consumer: its per-session queue is
full, or it holds at least a quarter of its own `WAR_QUEUE_BYTES` while the
global budget is exhausted. A session is never closed with `1013` because of
its recipients, and a recipient is never closed only because the global output
budget is exhausted.

Shutdown closes every QUIC connection with application error `1001`. A
connection refused before the WebTransport session is established (connection
limit, per-address limit, drain) is refused at the QUIC layer and has no session
close code. QUIC keep-alives are sent every `WAR_PING_INTERVAL_SECS`; a
connection that stays silent for `WAR_PONG_TIMEOUT_SECS` times out without a
close code. Exhausting the shared record staging budget
(`WAR_TOTAL_READ_BUFFER_BYTES`) ends the session with `1002`.
