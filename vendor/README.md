# Narrow WebSocket decoder patch

The relay uses the published `tungstenite` and `tokio-tungstenite` **0.30.0**
sources with three small changes exposing `reclaim_read_buffer()`. The native
connection can release unused decoder storage after a quiet input interval
without discarding a partially received frame, copying a published payload, or
resetting WebSocket protocol/fragment/control state.

Upstream's public APIs could not safely release this storage. Reconstructing a
WebSocket from its TCP stream loses buffered input and protocol state; a limit
checked after decoding does not control the decoder's allocation. This local
patch keeps the existing decoder and gives the connection owner explicit storage
control. No WebSocket framing, validation, masking, or output logic is replaced.

The only modified upstream source files are:

- `tungstenite/src/protocol/frame/mod.rs`: safe buffer replacement between frames.
- `tungstenite/src/protocol/mod.rs`: public synchronous forwarding method.
- `tokio-tungstenite/src/lib.rs`: public asynchronous-wrapper forwarding method.

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
buffer size and leaves fragment-assembly storage in place.

`tests/read_buffer_reclaim.rs` exercises shared-owner release, every masked
header split, deferred partial payloads, and a coalesced small message through
the public API. Native socket tests separately cover scheduling and routing.
Rebase the small patch deliberately when upgrading either package and rerun the
full relay/client/compatibility checks. Remove these copies when an upstream API
can provide the same ownership and partial-input guarantees.
