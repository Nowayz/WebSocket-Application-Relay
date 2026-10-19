# Prompt command cancellation on local client close — 2026-10-18

Selected: JavaScript `Relay.close()` rejects an outstanding command before requesting normal WebSocket closure. It reuses the pending command's existing rejection path, which clears its timer and releases the serialized command queue. Queued commands then encounter the existing closed-socket check. The normal Close code remains `1000`, and the socket's Close event still follows its ordinary lifecycle. No synthetic Close event, cancellation API, new state or dependency was added.

Previously an explicit local close left the command pending until a peer Close event or its request timeout. A delayed/nonresponsive peer could therefore leave both the current command and subsequent serialized commands waiting even after the caller had closed the client. Valid remote replies and remote Close handling remain unchanged. This is a client failure/lifecycle improvement; no server throughput or latency claim is made.

The new controlled-socket regression holds the socket in CLOSING without delivering a Close event, starts one command and queues another, calls `close()`, then checks both rejections on the next event-loop turn. It verifies normal Close code `1000` while the socket is still CLOSING. The original implementation fails immediately. The fixture emits its deferred Close in cleanup so even the failing test leaves no request timer waiting.

All 14 focused client tests pass in 69 ms. The complete 21-test set passes against disposable native and legacy-bridge processes in 1.09 seconds, including the existing one-second idle case and concurrent serialized commands. Both processes exit zero, with final connection/session/queued-byte/variable-byte gauges at zero. JavaScript syntax and diff checks pass, and the browser bundle rebuilds. No browser UI was executed for this change; controlled EventTarget and Node WebSocket integration are the runtime evidence.

`build.json` pins the source base, exact client/test sources and generated bundle. The selected patch, original failing test log, passing test log, live fixture/receipt/server logs and browser build log are retained. The live receipt pins its native and bridge executables; its native binary includes the selected management path-rejection fix. Run `python docs/benchmarks/2026-10-18-js-local-close/verify.py` to audit the evidence without launching services.

The browser bundle in `target/browser` is refreshed. No native/bridge convenience binary or running service was replaced. The three pre-existing dirty source files are preserved byte for byte.
