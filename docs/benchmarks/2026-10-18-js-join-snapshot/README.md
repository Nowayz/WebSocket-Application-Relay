# Capture subscription channels before asynchronous connection setup

Selected: Relay.connect() prepares its existing CBOR subscription frame immediately after validating the channel, before creating the WebSocket. The open handler sends that prepared frame. Previously it retained the caller's mutable channel until the later open event, so reusing a byte array could silently join another room, and transferring its backing buffer could fail encoding. This affects byte-array/subarray channels; strings are already immutable.

The runtime change moves one encoder.encode(channel) call earlier and reuses its result. It adds no second payload copy, dependency, public option, protocol field or state machine. String channels retain their CBOR text encoding and byte channels retain byte-string encoding. Length and timeout validation still occur first. The channel remains limited to the existing 16 bytes. Ordinary application sends and command queue behavior are unchanged.

Reproduction before the fix:

- A subarray containing abc is passed to connect(), then filled with x before a mock delayed open. The old client sends xxx instead of abc.
- A channel's backing ArrayBuffer is transferred before open. The old client fails in the encoder on the detached view.
- Against the actual native relay, a six-byte binary channel containing 0 and 255 is mutated immediately after connect(). Authenticated channel inspection proves the old client joined the changed name instead of the supplied name.

All three regressions fail against the original client; full before/after logs are retained. The selected tests also hold two simultaneous joins, grow the shared encoder's storage with an unrelated 64 KiB send, and open the sockets in reverse order. Both prepared subscription frames retain their exact bytes. This guards the encoded-output lifetime assumption without adding a defensive copy to production code. The pinned cbor-x 1.6.6 normal encoding path returns a subarray from append-style output storage; it does not use the codec's reset/reuse options. The encoder source and package lock are archived.

The final source passes all 44 client tests in 1.104 seconds against separate disposable native and legacy bridge processes. This includes existing uint64 IDs, assignment ordering, malformed command results, timeout/Close handling and unchanged legacy-client compatibility. Final connection, session, queue, variable, read-buffer and output-capacity gauges are zero; both server processes exit zero. The final factored connect-only tests also pass independently. A native relay change is not required.

The browser ES module is rebuilt with the existing build script in 6 ms. A separate native test imports the generated artifact with Buffer absent during module loading, selecting the codec's browser Uint8Array allocation path, then uses Node's real WebSocket to verify the original binary room. This artifact check passes in about 65 ms. It is not an actual browser UI or rendering test. Source and generated-module hashes are pinned independently. No hosted deployment or manual server/bridge restart is performed.

Base revision 2315b3b. Existing manual services and the three pre-existing source overlays are untouched. Raw original/selected clients, all test sources, fixture launchers/commands/receipts/logs, generated browser module/source map, codec source and selected patch are retained. Run python docs/benchmarks/2026-10-18-js-join-snapshot/verify.py for an offline audit. Reproduce the full client suite with the recorded native/bridge fixture or provide WAR_TEST_URL, WAR_TEST_BRIDGE_URL and WAR_TEST_IDLE_ENABLED to npm test in clients/js with the documented fixture credentials. This is a correctness and input-ownership fix, not a throughput or latency claim.
