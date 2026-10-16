# WebSocket decoder storage patch

The relay uses the published `tungstenite` and `tokio-tungstenite` **0.30.0**
sources with explicit storage reclamation and admission APIs. The native
connection can release unused decoder storage after a quiet input interval
without discarding a partially received frame, copying a published payload, or
resetting WebSocket protocol/fragment/control state.

Upstream's public APIs could not safely release this storage. Reconstructing a
WebSocket from its TCP stream loses buffered input and protocol state; a limit
checked after decoding does not control the decoder's allocation. This local
patch keeps the existing decoder and gives the connection owner explicit storage
control. No WebSocket framing, validation, masking, or output logic is replaced.

`set_read_budget(ReadLease)` attaches shared admission before the first read.
Frame and fragment-assembly storage reserve twice their requested capacity before
growing. Large frames include one configured read chunk of bounded read-ahead in
that capacity. Old and new staging allocations can therefore overlap during growth.
Each connection retains separate high-water frame and fragment leases; ordinary
reads within those allowances perform no shared-budget atomics. A failed growth
permanently stops reads and permits a best-effort Close. `ReadLease` releases its
allowance on drop, including cancelled connections. Published payload owners are
outside this staging allowance and require separate consumer limits.

The modified or added upstream source files are:

- `tungstenite/src/protocol/frame/mod.rs`: preallocation admission and safe buffer
  replacement between frames. A budgeted growth copies only unfinished input;
  published `Bytes` keep their original owner and pointer.
- `tungstenite/src/protocol/read_budget.rs`: atomic shared allowance and RAII leases.
- `tungstenite/src/protocol/message.rs`: admission before binary/text fragment
  assembly grows, preserving incomplete UTF-8 codepoints.
- `tungstenite/src/error.rs`: distinct staging-admission error.
- `tungstenite/src/protocol/mod.rs`: synchronous API, persistent fragment allowance,
  and terminal handling of denied reads.
- `tokio-tungstenite/src/lib.rs`: asynchronous-wrapper forwarding methods.

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

`tests/read_buffer_reclaim.rs` exercises shared-owner release, every masked
header split, deferred partial payloads, and a coalesced small message through
the public API. `tests/read_budget.rs` covers concurrent admission, refusal before
body allocation, terminal denial, fragment/UTF-8 assembly, shared payload pointers,
and reclaim/drop release. Native socket tests separately cover scheduling,
HTTP/Close admission responses, routing and cancellation/shutdown cleanup.
Rebase the patch deliberately when upgrading either package and rerun the
full relay/client/compatibility checks. Remove these copies when an upstream API
can provide the same ownership and partial-input guarantees.
