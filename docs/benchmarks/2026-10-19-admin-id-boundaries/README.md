# Exact management targeting across uint64 boundaries

Selected: add actual-socket regression coverage for the existing integer-preserving administrative disconnect API. The handler already passes; production code is unchanged. The new test protects against precision loss causing an administrative command to close a neighbouring user.

The existing malformed-ID test used an empty registry. This fixture runs the real two-worker native relay and HTTP management router with ephemeral loopback listeners and six authenticated, explicitly assigned WebSocket sessions across three sequential pairs:

| Boundary | Target ID | Live neighbouring ID |
| --- | ---: | ---: |
| Above uint32 | 4294967296 | 4294967295 |
| Above JavaScript exact integer range | 9007199254740993 | 9007199254740992 |
| Highest valid session ID | 18446744073709551614 | 18446744073709551613 |

For each pair, the fixture verifies native welcome, authentication and exact assignment acknowledgements; an unauthorized management POST returns 401 and leaves the target usable; old/absent IDs return 404; an authorized POST to the exact target returns 204. Only that target receives Close 4000 / Administrative Disconnect. The live neighbour receives an exact full-width departure ID, then successfully exchanges a private self-echo. A repeat POST to the retired ID returns 404, and the neighbour closes normally.

The fixture checks final sessions/root-channel census, connections, queued bytes and read reservations, then shuts down and joins both services and checks native arena/output capacity retirement. A five-second outer guard bounds the whole case. JoinSet and a cancellation drop guard own cleanup if an assertion fails.

## Verification

- The new fixture passes in 0.01 seconds against the unchanged production handler.
- A separate copied mutant changes only relay.disconnect(id) to relay.disconnect((id as f64) as u64). The identical test fails in 0.01 seconds at target 9007199254740993: the intended target does not receive Administrative Close. This demonstrates that the test catches the precision regression rather than merely checking successful HTTP responses. The mutant is not selected.
- All 138 library tests pass in 10.02 seconds. Existing real ten-second bridge timeout tests dominate that duration; no new slow test was added.
- Strict library/test Clippy passes in 2.16 seconds; formatting passes.
- The production portion of src/admin.rs before cfg(test) is byte-identical to base f86e483, independently checked in the offline verifier. No release/runtime change or performance improvement is claimed.

An initial version of the fixture forgot the normal departure event before the neighbour's echo. That incomplete oracle failed on the expected departure envelope. The accepted version explicitly verifies that event's exact ID, then the echo. initial-oracle.log is retained and is not evidence of a production failure.

## Scope and reproduction

Clean copied Cargo snapshots exclude the three pre-existing dirty relay overlays. The selected source contains only the new cfg(test) fixture. The isolated mutant adds its one bad production expression. Compressed base/selected/mutant sources, patches, source provenance and logs are archived. Existing services, convenience executables and unrelated dirty files were preserved. There was no need to rerun traffic benchmarks or rebuild release services for a test-only addition.

```sh
cargo test --locked \
  --manifest-path artifacts/admin-id-boundaries-20261019-source/Cargo.toml \
  --target-dir artifacts/bridge-buffer-check --lib \
  disconnect_targets_exact_uint64_sessions_and_preserves_neighbours
```

Build/check snapshots serially, refreshing copied source mtimes before a shared Cargo cache is reused. The full accepted suite was recompiled after the mutant, and passes this exact regression, preventing the bad binary from being accepted through cache reuse. `python docs/benchmarks/2026-10-19-admin-id-boundaries/verify.py` audits saved sources and outcomes offline.

Management URLs carry decimal uint64 IDs. JavaScript tools should retain IDs as BigInt and use id.toString() when constructing the URL. The native router already parses integers directly; the fixture preserves that contract at representative boundaries, without claiming exhaustive enumeration of every ID or a concurrency race test.
