# Validate bridge upstream configuration before admission — 2026-10-18

Selected: `legacy_bridge::validate_upstream` checks URL syntax and the WebSocket scheme through the existing Tungstenite client parser. The bridge CLI calls it before binding its listener, and `serve_with_limits` enforces it for embedders before creating the accept-loop state. The public validator performs no DNS resolution or network connection. Reachability and compiled TLS support remain checks made while forwarding a client; syntactically accepting `wss` is not proof that the current build supports TLS.

Previously invalid upstream syntax/schemes were discovered only after a legacy client joined and the forwarding connection was attempted. The bridge could enter its accept loop with that invalid configuration. A library regression reproduces this with a pre-cancelled service token: the old implementation returns success for an invalid URL because it never reaches a client, while the selected implementation rejects the configuration before entering the loop. The CLI now reports the configuration error before trying to bind. Normal per-connection request creation, connection deadlines, payload conversion, queue ownership and admission limits are unchanged. This is startup/failure behavior, with no forwarding performance claim.

Validation is short and focused:

- The regression fails immediately against the old implementation.
- All 92 working-tree library tests pass in 0.48 seconds, including invalid URL text, HTTP scheme, missing host and relative path. A separate synchronous check accepts valid IPv4, IPv6/path/query and secure WebSocket URLs without connecting to their addresses.
- Strict all-target Clippy, formatting and the clean release bridge build pass.
- Ten release CLI cases use an occupied frontend address: four invalid URLs and one valid URL, each against old and selected bridges. Old cases report a bind error first. Selected malformed cases report `invalid legacy upstream URL`; the valid case still reports the bind failure. This verifies configuration checking precedes binding.
- The complete 21-test client set passes with the new disposable bridge and selected native server in 1.10 seconds, including the existing one-second idle case. Both processes exit zero; final connection/session/queued-byte/variable-byte gauges are zero.

The validator reuses the client parser and scheme classifier, avoiding a separate URL grammar or a new dependency. The CLI and public serving API both enforce the same validator; the CLI therefore validates again when it enters the library, only during startup. No startup dependency on upstream availability was introduced.

`build.json` pins the source base, exact selected sources, clean snapshot and frozen old/new bridges. The source patch, original failing regression, test/build logs, CLI fixture/receipts and live client fixture/receipt/server logs are retained. Run `python docs/benchmarks/2026-10-18-bridge-upstream-validation/verify.py` to audit hashes, source identities, startup ordering and live compatibility offline.

No running service or convenience executable was replaced. The three pre-existing dirty source files are preserved byte for byte.
