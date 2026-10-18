# WebSocket decoder storage and scheduling patch

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

`python3 scripts/check-vendor.py` verifies both original archive checksums,
applies the declared patch in a temporary directory, and compares every resulting
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


## Buffered multicast classifier

The native relay can opportunistically group up to four already-buffered large
ordinary-room broadcasts. `WebSocket::buffered_binary_prefix` is a non-consuming
server-side classifier: it examines only a complete masked binary frame already
in the codec, rejects control/fragment/reserved-bit/incomplete states, and checks
a small unmasked prefix without allocating or fetching socket input. The normal
frame and CBOR parsers still validate and decode every accepted message. No
payload is copied by this classifier. The relay stops collection at membership
or observer changes and charges retained bodies through its existing budgets.
