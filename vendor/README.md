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
