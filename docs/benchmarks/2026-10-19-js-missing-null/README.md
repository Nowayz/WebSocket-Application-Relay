# Validate missing-variable replies in the JavaScript client

Selected: reject variable replies [false, value] unless value is exactly null. The documented native protocol returns [false, null] for a missing entry and [true, null] for a stored null. The bridge already enforces the missing-null schema; the JavaScript client previously checked only array length and boolean flag, allowing contradictory missing values to resolve a command successfully.

The runtime change adds one null comparison to the existing opcode-200 validation. Failure uses the existing pending rejection, error event and application Close 4002 / Invalid Relay Response path. No queue, timer, request ID, copying, dependency or normal forwarding behavior is added. Found values remain unmodified, including null, false, zero, empty text and structured values.

## Focused reproduction and verification

Two new deterministic tests cover seven malformed missing-value cases (undefined, false, zero, empty text, array, object and empty bytes) plus seven legitimate found/missing tuples. The invalid matrix checks immediate rejection of both pending and queued commands, zero queued wire sends, code 4002 and readyState=CLOSING before any peer Close acknowledgement. The positive matrix distinguishes missing null from stored null and preserves legitimate typed results. The first malformed case fails against the old client because the command resolves rather than rejects; the positive test already passes. [Before log](before.log).

The existing raw HTTP/WebSocket fault peer also sends valid CBOR [0, [200, [false, 0]]] in response to a real getVariable request. Before the fix, the intended rejection is absent. After the fix, both the active get and queued command reject, the peer observes client Close code 4002, and only one command reaches the wire. The fixture uses Node's actual WebSocket API and a local raw server, not a mocked close implementation. [Real before log](real-before.log).

Accepted checks:

- All 53 local client tests pass in 0.102 seconds, including the malformed-peer WebSocket checks. [Log](offline.log).
- The supported complete checker passes 68 JavaScript tests against clean native/bridge sources in 1.113 seconds of test execution. All six checked resource gauges retire to zero. Existing actual-relay missing-variable and typed-value tests remain green. [Log](js.log).
- The standalone browser ES module builds. Its source map contains the exact selected relay.mjs, independently compared and hashed in bundle.json. This verifies bundle construction and source inclusion; browser execution was not repeated for this validator-only change. [Build log](browser-build.log).

The Rust sources in the copied integration snapshot are byte-identical to base a08afbf and exclude the three pre-existing dirty overlays. Only three JavaScript files differ, plus documentation. There is no native runtime/performance change or new relay benchmark claim. Existing manual services and the currently served browser demo bundle were preserved; the browser build lives in this task's isolated fixture snapshot.

## Reproduction and evidence

```sh
node --test clients/js/connect.test.mjs clients/js/ids.test.mjs clients/js/protocol-error.test.mjs
```

From a clean selected snapshot with the root original relay.js included, scripts/check-js.sh rebuilds fresh disposable native/bridge executables and runs the complete suite. Keep Cargo users serial when sharing a target directory. npm run build:browser --prefix clients/js writes the standalone module into that snapshot's target/browser directory.

Compressed original/selected source and test files, patch, provenance, failed-before and accepted logs, and bundle hashes are retained. Run `python docs/benchmarks/2026-10-19-js-missing-null/verify.py` to audit the saved evidence; add --check-local-bundle to recheck the original isolated browser output and source map when those artifacts remain available.
