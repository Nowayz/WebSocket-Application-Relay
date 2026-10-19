# Reject direct recursive Relay serialization

Selected: one guard at the existing JavaScript encode() entry point rejects an attempt to serialize while another Relay encoding is active. The codec shares mutable output state. Before this change, a value getter calling send() emitted a nested packet and corrupted the outer encoding. Calling connect() from a getter could also reenter encoding before creating its socket. A known codec constraint now fails locally rather than generating invalid protocol bytes.

The runtime diff is one condition throwing Error("Relay serialization cannot be reentered") before incrementing the existing encoding counter. Direct send()/broadcast() fail synchronously, and async connect() returns a rejected Promise before constructing a WebSocket. An uncaught nested send error aborts the outer local send before either packet reaches the socket. The encoder's existing finally block restores its state, and subsequent normal sends work on the same connection. A rejected async connect can be handled while the outer packet remains valid. This wrapper guards Relay's own encoder entry points; it is not a global reentrancy mechanism for unrelated third-party codec use.

The existing queued-command clone/defer path remains enabled. Such commands can wait for the codec to become free and still retain FIFO/input capture; direct application sends cannot wait without changing their synchronous API. No new queue, copy, lock, dependency, option or wire field is added. Pure getters and ordinary application/command traffic retain their behavior.

Three regressions fail against baf5a39: nested application send must fail before any socket send; nested connect must reject before creating a socket without damaging the outer packet; and the actual native connection must recover after the local rejection. The mock tests also verify no Close request is made and that later sends work. An isolated diagnostic using the archived original client against native also
verifies the actual wire result: the nested self-message arrives, followed by a
server protocol Close 1002 for the corrupt outer packet. Its baseline source
hash is recorded independently, and its own final six resource gauges are zero.
The selected native regression instead receives a valid recovery message on the
same still-open connection after the local error.

The final 55-test native/bridge suite passes in 1.113 seconds, including the prior queued snapshot/reentrant command/timeout/error/Close and legacy compatibility coverage. Final connection, session, queued-byte, variable, read-buffer and output-capacity gauges are zero, and both disposable server processes exit zero.

The browser module is rebuilt. Its test imports the generated artifact with Buffer absent during module initialization, exercising the codec's browser allocation path, then uses Node's real WebSocket against native. Both the new rejection/recovery case and the prior queued mutation/transfer case pass. This verifies generated-module behavior, not a browser UI or rendering surface. No manual service is restarted or binary promoted.

A separate four-process, CPU-8-pinned SDK screen uses before/after/after/before order, 20,000 warmup calls and three timed rounds of 100,000 ordinary 1 KiB sends per process into a counting sink. Source hashes and all raw rounds are retained. Process medians are 374.89/367.29 ns before and 391.54/370.08 ns after; encoded-byte totals match at 331,840,000 per process including warmup. Ranges overlap, with a somewhat slower candidate repeat. These short samples do not establish general performance equivalence, and this correctness fix is not presented as a speedup. No network transport, backend throughput or 100,000-user latency claim follows from the SDK-local screen.

Base revision baf5a39. Raw original/selected client sources, selected patch, all tests, fixture launchers/receipts/logs, browser module/source map, build log and source-pinned timing harness/results are archived. Run python docs/benchmarks/2026-10-18-js-serialization-reentry/verify.py for an offline audit. Existing manual services and the three pre-existing source overlays remain untouched. No native or C++ runtime code changes.
