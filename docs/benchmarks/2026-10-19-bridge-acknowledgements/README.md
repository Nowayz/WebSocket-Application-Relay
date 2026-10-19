# Validate suppressed upstream acknowledgements — 2026-10-19

The bridge discarded upstream acknowledgements for operations 0, 1, 3 and 4 without decoding their result. Correct acknowledgements must remain invisible to original legacy clients, but malformed fields should not silently pass as valid replies. The selected change checks each field against the existing [command contract](../../protocol.md#commands) before discarding it:

| Operation | Required result |
| --- | --- |
| Authenticate (0) | Boolean, including false for failed authentication |
| Listener flags (1) | Unsigned value 0–7 |
| Assign own ID (3) | Unsigned 64-bit value other than zero or BROADCAST |
| Set variable (4) | Boolean true |

No acknowledgement is newly emitted to legacy clients. There is no new state, queue, lock, dependency, setting or allocation strategy. Native command handling, shared payload ownership and existing reply ordering remain unchanged. Malformed acknowledgements use the existing unsupported-upstream-reply Close path, 1003 / `Value Requires CBOR Client`; no new Close policy is introduced. This is contract validation, with no performance claim.

Two focused tests exercise 20 valid and 35 invalid result vectors. They retain false/true authentication, every effective flag combination, assigned IDs across encoding-width boundaries through u64::MAX-1, and successful variable writes. Invalid vectors cover scalar types, collections, out-of-range flags, reserved IDs and a false variable-write result. Against the original conversion path the valid test passes and the invalid test fails. After the fix, all 125 library tests pass in 0.53 seconds, warnings-denied bins/lib/test-target Clippy passes, and all 65 JavaScript protocol/legacy/idle checks pass in about 1.11 seconds with six checked resource gauges retired.

A separate real-wire controller runs each of the four opcodes with a valid or malformed acknowledgement against both frozen release binaries: 16 disposable case processes. Its controlled upstream receives exact subscription/authentication/command/variable-query bytes, then pipelines a valid authentication acknowledgement, the target acknowledgement and a valid missing-variable reply in order. For valid cases, both builds hide both acknowledgements and deliver only the expected nine-byte empty variable reply before normal Close/EOF. The old build also silently hides the malformed target acknowledgements; the selected build instead completes the existing frontend rejection Close/EOF path. All cases return descriptors from 10 to 10, retire the upstream, finish controller threads and exit the bridge zero without forced termination. Backend retirement on rejection can be EOF or reset; accepted cases complete the backend Close handshake.

The controller is a schema/ordering fixture with a synthetic upstream, not a test of backend authentication authorization or real variable storage. Existing native/legacy integration tests retain those valid-flow checks. It makes no throughput, population-capacity, tail-latency or long-duration stability claim. All builds and wire checks run sequentially; no large-population test is needed for these bounded result fields.

The control is the previous selected missing-variable-validation release. The candidate is a fresh clean snapshot with exact input hashes and `fresh:false` in its build manifest; the selected working source matches it. Original/selected sources are gzip-compressed alongside their patch, complete manifests, test logs and wire receipts. Existing services, convenience binaries and three unrelated worktree edits are preserved.

From a clean checkout, run `cargo test --locked --lib upstream_acknowledgements` for the focused checks. From the repository root, `probe.py --server PATH --output FRESH.json` reproduces all eight wire cases for one binary, using fresh loopback listeners, Linux descriptor checks and only owned child processes. The probe records observed frames for either build; `verify.py` enforces the expected old/new result and audits sources, hashes, field vectors and all 16 saved cases without starting a service.
