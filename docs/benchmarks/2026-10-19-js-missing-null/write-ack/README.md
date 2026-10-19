# Require a true variable-write acknowledgement

Follow-up to [missing-variable reply validation](../README.md). The native protocol specifies true for opcode-4 write acknowledgement; false is a legitimate authentication result at opcode 0. The bridge already requires true for a suppressed write acknowledgement. The JavaScript client previously accepted both booleans for both operations.

Selected: one guard rejects opcode-4 results other than true, after the existing boolean-type validation. Nonboolean replies retain their previous Invalid boolean response error. A false write acknowledgement now rejects with Invalid variable acknowledgement through the existing protocol-error path, closing with application code 4002 and preventing queued commands from reaching the wire. No task, queue, timer, payload copy or dependency is added. Authentication false, successful writes and false/null stored values remain supported.

## Evidence

The new deterministic fixture starts a write followed by a queued read, then supplies an opcode-4 false reply. It checks that neither result is accepted, the queued read is not sent, code 4002 is requested, and both commands settle while the socket remains CLOSING without peer acknowledgement.

The raw real-WebSocket fault peer independently returns CBOR [0, [4, false]]. The command must reject, the next command must stay off the wire, and the peer must receive Close 4002. Both new regressions fail against the original client, with missing expected rejection / acceptance instead of error. [Before log](before.log).

The existing positive result test formerly treated false as valid for writes as well as authentication. Its accepted-result table now explicitly covers authentication false, authentication true and write true, using the public helpers. Its arbitrary stored-value checks remain. This corrects the test oracle to the documented schema and is paired with the new negative regression.

Accepted checks:

- 55 local client tests pass in 0.097 seconds, including both new fixtures and the authentication distinction. [Log](offline.log).
- 70 complete native/bridge interoperability checks pass in 1.118 seconds; all six checked resource gauges retire to zero. [Log](integration.log).
- The browser module builds, and its source map contains the exact selected relay.mjs. Bundle construction/source identity is verified; a separate browser execution is not claimed. [Build log](browser-build.log).

The clean integration snapshot is based on 5c31c52, with exactly the four changed JavaScript files copied in. Every Rust source is byte-identical to that commit, excluding the three pre-existing dirty overlays. No native runtime or performance change is claimed. Existing manual services and served browser bundle remain unchanged; the browser output is isolated under the fixture snapshot.

## Reproduction

```sh
node --test clients/js/connect.test.mjs clients/js/ids.test.mjs clients/js/protocol-error.test.mjs
```

The supported scripts/check-js.sh performs the complete disposable native/bridge run. Include original root relay.js in a fresh clean snapshot and keep Cargo builds serial. Compressed selected module, patch, source hashes, failure/passing logs and bundle metadata are archived. The original module is retained in the parent report's selected-relay.mjs.gz and its identity is independently checked. Run `python docs/benchmarks/2026-10-19-js-missing-null/write-ack/verify.py`; optional --check-local-bundle rechecks the original local bundle and source map.
