# Protocol storage and scheduling patches

## QUIC connection event storage

Published `quinn` **0.11.12** uses one endpoint-to-connection event channel per
connection. The local patch keeps the first event inline and grows an overflow
queue only when events accumulate, avoiding an initial 32-event block for each
idle connection. There is one endpoint-owned sender and one connection-owned
receiver. The endpoint continues to serialize producers. FIFO, close/rebind
events, pending-reader wakeups and receiver retirement remain explicit; no
channel-depth cap or event-discard policy is introduced.

After a large burst drains, oversized overflow storage is released. Small buffers
are reused up to the original mpsc block-retention size (32 slots on 64-bit
targets, 16 otherwise); this threshold controls reuse, not queue depth. Receiver
and sender retirement also recover a poisoned queue lock during unwinding so
cleanup releases retained events without causing a second panic.

Receiving consumes Tokio's cooperative budget as the original mpsc receiver did.
The public `poll_proceed` API was introduced in Tokio 1.47, so the patched manifest
declares that minimum and enables `rt` for the API. The relay already uses Tokio
1.53.3 with `rt`. Outside a Tokio budget context the API is unconstrained, allowing
the channel to retain its executor-independent polling contract.

The modified package files are `Cargo.toml`, `Cargo.toml.orig`, `src/lib.rs`,
`src/endpoint.rs` and `src/connection.rs`; `src/event_queue.rs` and
`src/transmit_buffer.rs` are added. The
published archive, licenses and remaining source files are preserved. The complete
diff is `quinn-event-channel.patch`, with original checksum and Git revision in
`provenance.json`. Tests exercise FIFO/bursts, concurrent wakeups, cancelled polls,
sender/receiver retirement, and cooperative scheduling alongside the upstream
runtime tests and real relay integration workloads.

## Empty sent-packet tree reclamation

`PacketSpace::take` releases the empty `BTreeMap` root after removing the last
sent packet. Upstream retains that leaf even when the connection becomes idle;
at 100,000 sessions the measured packet-node allocation site retained about
116 MB. Packet-number, ACK, loss and deduplication history remain unchanged.
The next outstanding packet recreates the tree. This trades a small measured
throughput cost for lower idle memory; see the empty-packet-tree report. A
worker-local empty-tree cache was tested and rejected because it did not improve
the complete mixed workload.

## Drained endpoint-event queue reclamation

`Connection::poll_endpoint_events` releases the queue's storage when it pops the
last event. Upstream keeps the handshake burst's four-slot buffer (192 bytes) in
every connection for its whole lifetime, although the queue only carries
control-plane events: reset tokens, connection-ID issue/retirement and drain.
A conforming peer's data packets produce none, so the cost is one small
allocation per later burst (about three per handshake instead of one retained).
Order, producers and the endpoint's handling are unchanged; no depth limit or
coalescing is introduced. `drained_endpoint_events_release_storage` checks both
peers after a real handshake, alongside the upstream CID rotation and retirement
tests. Ready RSS at 100,000 sessions falls by about 19 MiB. The change is part of
`quinn-sparse-streams.patch`.

## One-slot first receive-assembler growth

`Assembler::insert` reserves one slot, not the default four, when it buffers the
first chunk of a stream. A stream read as fast as it arrives holds one chunk at
a time, and the two HTTP/3 control streams of every session receive once and
then stay idle; upstream leaves each of them a 224-byte, four-slot heap holding
nothing. A stream that ever queues a second chunk grows to four slots through
the ordinary path and ends in the upstream state after one extra 56-byte move in
its lifetime. Nothing is released on read, so steady traffic allocates exactly
as before. This is not the inline-first-chunk design that was measured and
rejected twice. Ready RSS at 100,000 sessions falls by a further 50 MiB with
unchanged throughput; see
`docs/reports/webtransport-allocator-idle-storage-2026-11-13.json`.

## Sent handshake control-frame queues

`populate_packet` releases the Data-space `crypto`, `new_cids` and `new_tokens`
pending queues when the frame it has just written leaves them empty. A server
fills each once around the handshake (session tickets, the first connection-ID
issue, one address token) and upstream keeps their 480 bytes of storage for
the life of the connection. The release sits on the path that has just sent
such a frame, so packets without them run no new code; a truncated CRYPTO
frame that is pushed back keeps its queue. Retransmission bookkeeping in
`SentPacket` is unchanged. `sent_handshake_control_frames_release_pending_storage`
checks both peers after a real handshake.

## Boxed previous path

`Connection::prev_path` is `Option<Box<(ConnectionId, PathData)>>`. Only a
connection that has migrated and is still validating holds one; inline it cost
every connection 432 bytes. The per-packet check in `can_send_1rtt` is the same
null test, and migration allocates once. With jemalloc this moves the
connection owner from the 6,144-byte to the 5,120-byte size class. An earlier
out-of-line layout was rejected under the system allocator, where the saving
was 41 MiB at 100,000 sessions; see the 2026-11-13 report for the present
measurement. Upstream migration and path-validation tests cover the paths.

## QUIC packet-construction storage

The same Quinn runtime patch reuses packet-construction buffers per runtime
thread instead of retaining their high-water allocation in every connection.
A synchronous transmit turn owns a `TransmitBuffer` lease. After successful
socket submission, its cleared allocation returns to that thread for the next
connection. One cached allocation per thread replaces per-idle-connection storage;
no queue depth, message size, flow-control window or congestion setting changes.
Nested/custom socket callbacks borrow an independent buffer. Thread teardown and
an already borrowed cache fall back to releasing the allocation without panicking.

An unsent packet is different: when socket writability returns Pending, the
connection keeps the exact encrypted bytes and transmit metadata until retry.
That allocation is never placed in the shared cache while the socket is blocked.
Existing socket error handling, transmit quanta, timers and protocol ownership
remain unchanged. This does not share a mutable buffer across async waits.

`tests/quic_transmit_buffer.rs` wraps a real UDP socket to force WouldBlock and a
subsequent Pending poll. It constructs a second connection's packet on the same
runtime thread before releasing the first, then checks the first packet's exact
ciphertext and both delivered payloads. Upstream runtime/window tests, relay
integration workloads and actual Chromium checks provide broader verification.
The complete archive-relative patch remains `quinn-event-channel.patch`.

## QUIC stream-credit storage

The `quinn-proto` **0.11.19** sources retain upstream stream credits and stream
lifecycle behavior while representing unused remote stream IDs as compact ranges.
Upstream creates hash-map placeholders for every remotely available stream credit.
`SparseMap` materializes a placeholder when mutable stream state is needed; immutable
lookup still distinguishes a valid unopened stream from a closed or unknown one.
Locally initiated and initialized stream state retains its ordinary hash-map owner.
Transport-parameter updates visit initialized send state without materializing empty
placeholders. No advertised stream counts, flow-control windows, congestion
control, or stream validation rules are changed.

When Initial or Handshake packet keys retire, the connection also releases pending
crypto retransmission storage and the crypto assembler staging heap. Consumed
offsets and packet-number/ACK history remain intact; the Data packet space is
unchanged. Retry rebuilds Initial state using the saved ClientHello and existing
next packet number, as upstream does.

Pending retransmits use the existing `ThinRetransmits` representation, as sent
packets already do. Mutation explicitly calls `get_or_create`; empty reads and
merging an absent queue allocate nothing. A merge into an absent destination moves
the existing box, while merging two populated queues retains upstream CRYPTO
retransmission priority. Retiring a packet space releases its pending box; scalar
packet-number, ACK, deduplication and assembler history remain inline.

The modified upstream files are `src/connection/streams/mod.rs` and
`src/connection/streams/state.rs`, plus `src/connection/mod.rs` and
`src/connection/assembler.rs` and `src/connection/spaces.rs`; `src/connection/streams/sparse.rs` is added.
`quinn-sparse-streams.patch` is the complete diff against the verified published
archive. `provenance.json` records the package checksum and upstream Git commit.
All other archive files, including licenses, are retained unchanged.

The dependency's protocol tests and the relay's real UDP/TLS integration tests
exercise the change. Additional map tests compare mixed operations with an
independent standard hash map and confirm that unused stream credits need no hash
slots. Performance and resource results, including unresolved parity requirements,
are recorded in `docs/webtransport-pivot.md` and its linked benchmark reports.

## WebSocket decoder storage and scheduling

The relay uses the published `tungstenite` and `tokio-tungstenite` **0.30.0**
sources with explicit storage reclamation, admission and bounded parsing APIs. The native
connection can release unused decoder storage after a quiet input interval
without discarding a partially received frame, copying a published payload, or
resetting WebSocket protocol/fragment/control state.

Upstream's public APIs could not safely release this storage. Reconstructing a
WebSocket from its TCP stream loses buffered input and protocol state; a limit
checked after decoding does not control the decoder's allocation. This local
patch keeps the existing decoder and gives the connection owner explicit storage
control. Bounded parsing also lets asynchronous callers yield between fragments.
No WebSocket framing, validation, masking, or output logic is replaced.

The native readiness runtime enables `WebSocketConfig::shared_read_buffer(true)`.
This defers its initial receive allocation. `swap_read_storage` lends empty
worker storage to a decoder; `take_read_storage` returns it between complete
frames. Unread header bytes, a parsed header awaiting its body, partial payloads,
fragment assembly, and protocol/control state stay with their connection.
An empty decoder retains a zero-byte lease, so lending its allocation away can
never make its next growth unbudgeted. Different admission pools cannot exchange
storage. Published `Bytes` retain their immutable owner and original pointer.

`ReadStorage` carries both the allocation and its lease. It also records the
backing capacity before splitting off published views; the tail's available
capacity alone cannot identify a small view of a large slab. The runtime uses
`read_storage_capacity` for its compaction decision, and calls
`discard_if_larger_than` on oversized empty storage before lending it again.
This drops only the staging owner's view and lease, without copying published
bodies. Default configuration preserves upstream's eager allocation behavior.

`set_read_budget(ReadLease)` attaches shared admission before the first read.
Frame and fragment-assembly storage reserve twice their requested capacity before
growing. Large frames include one configured read chunk of bounded read-ahead in
that capacity. Old and new staging allocations can therefore overlap during growth.
Retained storage has separate high-water frame and fragment leases; ordinary
reads within those allowances perform no shared-budget atomics. A failed growth
permanently stops reads and permits a best-effort Close. `ReadLease` releases its
allowance on drop, including cancelled connections. Published payload owners are
outside this staging allowance and require separate consumer limits.

The modified or added upstream source files are:

- `tungstenite/src/protocol/frame/mod.rs`: preallocation admission and safe buffer
  replacement/lending between frames. A budgeted growth copies only unfinished input;
  published `Bytes` keep their original owner and pointer.
- `tungstenite/src/protocol/read_budget.rs`: atomic shared allowance and RAII leases.
- `tungstenite/src/protocol/message.rs`: admission before binary/text fragment
  assembly grows, preserving incomplete UTF-8 codepoints.
- `tungstenite/src/error.rs`: distinct staging-admission error.
- `tungstenite/src/protocol/mod.rs`: synchronous APIs, persistent fragment allowance,
  terminal handling of denied reads, and bounded fragment parsing.
- `tokio-tungstenite/src/lib.rs`: asynchronous-wrapper forwarding methods and
  explicit wakeup after a bounded parsing step.

`read-buffer-reclaim.patch` is the complete diff against the published sources.
`provenance.json` records original package hashes and upstream commits. The crate
archives were verified against their original `Cargo.lock` checksums, and the
cached source files were compared with those archives. Original package sources,
manifests, README files, and original license files are retained. `Cargo.toml` selects these
copies using `[patch.crates-io]`; no separately published fork is required.

`python3 scripts/check-vendor.py` verifies every declared archive checksum,
applies the declared patches in a temporary directory, and compares every resulting
file with these copies. It caches the original archives under `target` and needs
network access on the first run. CI runs this check so unrelated dependency
changes or a stale patch cannot pass silently.

The hook returns the number of incomplete header bytes copied, or `None` while a
frame or complete buffered header still needs processing. A short buffered item
may already contain a complete small message, so the normal header parser must
prove that it is incomplete before any bytes move. Existing published `Bytes`
owners keep their original storage. The hook allocates only the initial read
buffer size and leaves active fragment-assembly storage and its allowance in
place. An inactive fragment high-water allowance releases at this safe reset.
Setting no budget retains upstream's original growth behavior.

Budgeted binary/text fragment assembly caps intermediate geometric growth at the
already-checked message limit. If a final fragment requires growth, the decoder
uses the now-known complete length; it retains existing sufficient capacity.
For example, a 3 MiB prefix followed by a final 1 MiB grows assembly to 4 MiB
rather than doubling to 6 MiB, including under a 16 MiB configured message limit.
The actual-length check still precedes reservation and allocation. This also
applies below the usual eight-byte minimum growth, while preserving incomplete
UTF-8 state and published payload ownership. The no-budget path keeps upstream's
original allocation behavior.

`WebSocket::read_with_frame_limit(NonZeroUsize)` uses the same parser but returns
`Ok(None)` after consuming the configured number of frames without producing a
message. This means cooperative yield, not EOF or socket blocking. Ordinary
`read()` retains its unlimited behavior. `WebSocketStream::set_read_frame_limit`
opts into bounded polls; a yield explicitly wakes the caller and returns
`Pending`, so already buffered continuations resume even without a new I/O event.
The asynchronous default is unchanged (`None`). Native and legacy server sockets
enable 256 frames per poll. Incomplete data, UTF-8/control state, admission leases
and published payload owners remain in place. No extra payload buffer or
per-yield allocation is introduced by these hooks. This bounds frame count per
poll, not elapsed time, total CPU use or the work within a single frame.

`tests/read_buffer_reclaim.rs` exercises shared-owner release, every masked
header split, deferred partial payloads, and a coalesced small message through
the public API. `tests/read_budget.rs` covers concurrent admission, refusal before
body allocation, terminal denial, fragment/UTF-8 assembly, shared payload pointers,
and reclaim/drop release. Native socket tests separately cover scheduling,
HTTP/Close admission responses, routing and cancellation/shutdown cleanup.
`tests/read_fairness.rs` additionally checks bounded parsing, UTF-8/control
resumption, self-wakeup with entirely buffered input, dropped read futures,
shared payload pointers and terminal protocol errors.
Shared-storage tests additionally cover lending across decoders, every partial
masked header and payload split, admission after lending, and retirement of a
large worker view while published bodies remain alive, with admission enabled
and disabled.
Rebase the patch deliberately when upgrading either package and rerun the
full relay/client/compatibility checks. Remove these copies when an upstream API
can provide the same ownership and partial-input guarantees.

## QUIC-only TLS record cipher storage

The published `rustls` **0.23.46** source is pinned in `provenance.json` with the
archive-relative `rustls-quic-record-ciphers.patch`. QUIC uses TLS for its
handshake and derives separate packet and header-protection keys; it does not
send TLS records ([RFC 9001 section 4](https://www.rfc-editor.org/rfc/rfc9001.html#section-4)).
The upstream key schedule nevertheless constructs TLS record ciphers. The
patch uses rustls' existing fail-closed, zero-sized record cipher sentinels only
when `CommonState::is_quic()` is true. It preserves the active encryption state,
sequence reset and confidentiality-limit state: the active flag labels queued
QUIC handshake bytes. Server handshake decryption also takes this path when
TLS trial decryption would otherwise be requested. TCP/TLS takes the original
cipher derivation path. QUIC packet/header keys, handshake secrets, key logging,
authentication, key updates and crypto algorithm selection remain unchanged.

The focused unit test checks the active flag and rejection of accidental record
decryption. Run the full rustls library suite with the exact upstream fixtures:

```sh
python scripts/test-rustls-vendor.py --directory artifacts/rustls-unit-verification
```

Use a fresh directory. The published crate omits test data and fuzz corpus;
the helper verifies the upstream fixture archive checksum, copies the vendored
source into that directory, adds fixtures there and uses its locked dependencies.
It never writes fixtures or lockfile updates into the vendor tree. Relay,
QUIC key-update/0-RTT, browser and loss tests are separate acceptance checks.

### Consumed QUIC handshake input storage

The same rustls patch also releases the deframer input vector once the QUIC
handshake is complete, no handshake spans remain active, and the input is
empty. It preserves deframer ordering/progress, packet keys and TLS state.
Partial post-handshake messages retain their storage; later messages may
allocate again. The ordinary 100k comparison saved 37.68 MiB ready RSS.
Throughput rankings varied across cohorts, with lower room/mixed medians in
both longer cohorts; this is a memory tradeoff, not a throughput improvement.
See `docs/reports/webtransport-handshake-buffer-2026-10-22.json`.

### Endpoint event overflow scratch

The Quinn event-channel patch now shares empty overflow buffers through an
endpoint-owned pool, accessible to both the ingress and connection-driver
runtimes. A thread-local prototype had zero reuse because the two owners ran
on different threads. A CPU-sized shared cache reused only about half of
requests and discarded many returns. The retained cache is sized to one UDP
receive batch (`max_receive_segments * BATCH_SIZE`); diagnostic reuse was
99.99% private and 99.73% mixed with no full-cache returns.

Only empty small buffers enter the pool. The existing per-buffer retention
threshold remains; larger drained buffers free storage. The pool size bounds
retained scratch, never event count or queue depth. FIFO, wakers, cooperative
scheduling and retirement retain their queue ownership. Queue locks precede
pool locks; pool operations never acquire a queue lock. Ordinary ready RSS
fell by 81.66 MiB at 100k sessions. Throughput rankings varied between cohorts,
and high-rate datagram loss varied; see
`docs/reports/webtransport-event-batch-2026-10-22.json` for complete evidence.

### Final packet-space retirement

The quinn-proto patch stores Initial and Handshake packet spaces separately
from inline Data-space state. Final key discard removes in-flight packets,
recomputes the loss timer and releases the earlier space. Its absent slot is
a tombstone: lookups never allocate or recreate it. A valid Retry instead
clears the old recovery state and replaces Initial keys in still-live storage,
preserving the next packet number and existing Retry behavior.

This follows RFC 9002 section 6.4 for discarded-key recovery state; it does not
retire 0-RTT/Data history or alter stream offsets, Data ACK/dedup history,
congestion policy or key-update precomputation. Focused tests replay early
packets after confirmation and verify bidirectional application traffic.
Protocol Connection storage fell 5,728 to 4,656 bytes; the actual Quinn
connection allocation fell 6,416 to 5,344 bytes. Ordinary ready RSS saved
100.31 MiB at 100k sessions. Mixed throughput medians fell 2.05% initially and
1.10% in the longer cohort, so this is a documented memory/throughput tradeoff.
See `docs/reports/webtransport-packet-spaces-2026-10-22.json`.


Quinn allocates writable-poll notification state only after a nonblocking
send returns WouldBlock. Pending preserves the exact transmit buffer and
registers a wakeup; rebind drops the previous socket's notifier. A forced
backpressure/rebind test verifies progress and payload integrity. This saves
17.95 MiB requested live allocations at 100k sessions; longer throughput
medians change -1.21% private, +2.62% room, +2.30% mixed. Earlier short runs
were slower; timing and datagram loss vary. See
`docs/reports/webtransport-lazy-writable-2026-10-22.json` for scope and evidence.


Rustls QUIC handshake/application secret handoff values use temporary boxes
instead of large inline optional storage. Existing take/derive/update
transitions remain unchanged; packet and header ciphers and next-key
precomputation remain intact. QUIC server layout shrinks by 336 bytes and
100k ready RSS saves 30.63 MiB. Two temporary handshake allocations are the
tradeoff; longer mixed throughput fell 0.58% while private/room improved.
See `docs/reports/webtransport-boxed-quic-secrets-2026-10-22.json`.


Rustls QUIC also releases empty handshake-fragment span storage at the
completed-handshake/no-active-fragment/empty-input guard. Deframer progress
and ordering remain intact, and TCP/TLS retains its normal staging behavior.
This removes the 640-byte constructor allocation at 100k sessions and saves
62.89 MiB ordinary RSS. Longer throughput changes +0.93% private, -0.07%
room and -0.05% mixed; earlier short runs were worse. See
`docs/reports/webtransport-handshake-spans-2026-10-22.json`.


Quinn-proto stream ACK bookkeeping releases an ordered prefix directly when
there are no pending out-of-order ACK ranges. Other cases retain the original
range-tree algorithm. This removes the 300k x 192-byte range-tree site in the
100k ordered workload and saves 59.62 MiB ordinary RSS. Longer throughput
changes +0.14% private, +0.83% room, -1.14% mixed. Protocol, paced traffic,
loss/reordering and Chrome checks pass. See
`docs/reports/webtransport-ordered-ack-2026-10-22.json` for evidence and limits.

## QUIC early secret and handshake queue

`Quic::early_secret` is boxed: it is set only when a PSK handshake derives
0-RTT keys, and inline its 80 bytes pushed every established session shell
past 1,024 bytes. `ConnectionCommon::write_hs` releases the handshake message
queue once the handshake is complete and the queue is empty (128 bytes); later
post-handshake messages allocate it again for the moment they are queued. No
key schedule, derivation point or key count changes. Both are part of
`rustls-quic-record-ciphers.patch`.

## Early 1-RTT packets held during the handshake

A 1-RTT packet that arrives while the connection is still in the handshake
state used to be dropped; upstream marks this with a to-do to buffer it. On a
path that reorders, the first data packet regularly overtakes the Finished, so
the peer saw a loss on its first flight, cut its congestion window and left
slow start. `Connection::handle_decode` now copies such a packet into
`early_short_packets` when it comes from the handshake peer's address, up to
16 packets and 14,720 bytes (the largest initial window RFC 9002 allows), and
`handle_event` processes them once a datagram takes the connection out of the
handshake state. The copy keeps a held packet from pinning its receive
buffer; the list is released when replayed and on close. Packets beyond the
bounds, or from another address, are dropped as before. Held packets are
stamped with the time they are processed, so the hold is not reported in ACK
delay. The upstream test `handshake_1rtt_handling` covered this case and
asserted a loss; it now asserts none.

## Loss bursts for MTU black-hole detection

`detect_lost_packets` closed the black-hole detector's current loss burst on
every pass. The relay detects loss by the time threshold only, so packets
sent a few milliseconds apart in one loss event are declared by successive
timer passes; each would have counted as its own burst and four suspicious
bursts reset the path MTU for a minute. A burst now ends only on an
ACK-driven pass, and `BlackHoleDetector::on_non_probe_lost` tolerates a
smaller packet number from another packet number space arriving while a burst
is open. A real black hole is still found with the packet threshold disabled
(`blackhole_after_mtu_change_repairs_itself_without_packet_threshold`). The
false positive was identified by review and not reproduced.

## Connection-ID retirement after the packets that used it

`Connection::update_rem_cid` used to queue RETIRE_CONNECTION_ID for the
previous remote ID at once. A quinn server switches as soon as the client
issues its first new ID, while packets sent to the handshake ID may still be
in flight; if the retirement overtakes one, a peer that stops routing a
retired ID at once (quinn's endpoint does) drops it, and the sender declares
a first-flight loss. The retirement is now held in `deferred_retire_cids`
until no ack-eliciting Data packet numbered below the switch is outstanding,
and released by `retire_deferred_cids` from `detect_lost_packets`, which every
ACK reaches. Held-back ranges merge only when adjacent, and a peer's Retire
Prior To releases them at once. Tests:
`server_retires_initial_client_cid_after_its_packets` and
`deferred_cid_retirement_with_retire_prior_to`.

## Copying stream reads without splitting packet buffers

`RecvStream::poll_read_buf` used to take each chunk from the receive
assembler with `Chunks::next(buf.remaining())`. When the caller's buffer was
smaller than the chunk, as for the relay's 4-byte record-length reads, the
assembler split the chunk, which clones the shared packet buffer, and the
copy then dropped the clone: two atomic updates per record on a buffer the
ingress thread had just written. `Chunks::read_with` passes the head chunk's
ordered bytes to a closure and advances or pops the chunk in place, so a
copying read touches a reference count only when it finishes a chunk.
`next` and `read_with` share the stream-state handling. Under the historical
private-routing workload the read path's clone fell from 1.2% to 0.1% of
relay samples and its drop from 2.6% to 1.0%; CPU per delivery fell 1-4%.
Test: `read_with_copies_in_place_across_overlaps`. The quinn-proto part is in
`quinn-sparse-streams.patch` and the quinn caller in
`quinn-event-channel.patch`.

## UDP receive-batch timestamps

The Quinn endpoint samples time immediately after each successful socket
receive, before routing that batch. Reusing the start of a longer endpoint
poll could stamp fast ACKs earlier than concurrent packet transmissions and
turn negative RTT intervals into zero samples. The patch removes that stale
timestamp plumbing. A fake-clock stateless-reset regression distinguishes
after-receive sampling from both shared and before-receive timestamps.
The complete change remains in `quinn-event-channel.patch`.

## Established WebTransport session IO

Published `web-transport-quinn` **0.13.3** retains request/response metadata,
stream-opening headers, per-handle operation slots and acceptance machinery in
every session. The relay needs one application bidirectional stream, followed by
datagram IO and close handling. Its consuming `try_into_established` path splits
that IO from a temporary `SessionAcceptor`. Neither handle keeps the original
`Session` allocation alive.

After admission, `finish_accepting` releases empty acceptance machinery. It keeps
partially decoded extra streams, QPACK streams and acceptance state shared with
other Session clones until the IO handle drops. The relay calls it before
propagating either a join error or timeout, preserving ownership through the
outer error-close path. Settings streams, CONNECT-send ownership, background
capsule processing, session-ID framing and first-writer-wins errors remain owned
by the compact IO handle. It exposes no raw Quinn stream-opening methods.

Conversion refuses pending operation slots and HTTP/3 poll-accept waiters,
returning the original handle intact. `Parked` explicitly tracks a live park;
cloning starts empty and successful polls release it. This keeps consuming
conversion from silently cancelling a registration or an owned operation.

The complete archive-relative change is `web-transport-session-io.patch`.

The protocol's application-event queue stores four internal unit notifications
in a private one-byte enum and converts each to the unchanged public `Event`
when polled. Stream events and connection errors remain separately polled in
the original order. All notification producers, FIFO order and duplicates are
preserved; no queue-depth limit or notification coalescing is introduced.

The previous queue allocated storage for the 56-byte public enum even though
none of its queued variants carried a payload. At 100,000 connected sessions,
the observed retained buffers fall from 100,000 x 224 B to 100,000 x 8 B, with
the 5344-byte Quinn connection allocation unchanged. This measures the event
buffer component; full process allocations, RSS and throughput are reported
separately. The complete change is included in `quinn-sparse-streams.patch`,
and `src/connection/datagrams.rs` is declared in `provenance.json`.
`provenance.json` pins the published archive and the three modified source files.
Focused real-QUIC tests cover busy conversion, wake registration, raw/nonzero-ID
datagrams, empty-state release, partial/QPACK retention, clones and close capsules.

The matched 100k screen saves 161.49 MiB of requested live allocations and
192.34 MiB of ordinary RSS. These net totals include growth of the relay task
allocation from 1920 to 2048 bytes per session. Longer median throughput changes
are -0.37% private, +0.99% room and +0.61% mixed; they establish no statistical
speed claim. Browser, paced reliable/datagram, both-endpoint profile and user-space
loss/bulk checks pass. Full throughput and memory parity remain unmet; see
`docs/reports/webtransport-session-io-screen-2026-11-06.json` for measurements,
reproduction and fixture limits.
