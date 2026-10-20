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

Use the native relay and management addresses, ordinarily ports 1338 and 1339.
Remove bridge-specific services/configuration from your own deployment after its
clients migrate. Set independent relay and management secrets, configure browser
Origins and terminate TLS at your ingress. The repository's systemd and Caddy
examples describe the native service.

The project retains its MIT license and upstream attribution. Complete previous
implementations and evidence exist in Git history at `587c1cd`, for example:

```sh
git show 587c1cd:relay.js
git show 587c1cd:cpp/BENCHMARKS.md
git show 587c1cd:docs/benchmarks/2026-10-19-global-shutdown-indexes/README.md
```

The concise [performance history](reports/performance-history.md) retains the useful
findings without keeping obsolete binaries, source snapshots, charts and raw
benchmark dumps in the maintained tree. Future comparisons use previous Rust
revisions with identical native CBOR clients and workloads.
