# JavaScript connection setup cleanup — 2026-10-18

Selected: `Relay.connect` removes every temporary setup listener and clears its timer when setup succeeds or fails. Failure also requests socket closure. Encoding/sending the initial subscription is caught and rejects the connection promise with the original error immediately. No new dependency, retry loop or protocol state was added; established-message and server routing paths are unchanged.

Previously, an early error or Close cleared the timer and rejected the promise but left the open and welcome listeners attached. A delayed open could still send a subscription, and a delayed welcome could construct a Relay even though the original promise had rejected. The timeout path also left those listeners attached. A synchronous exception from the open handler's join send escaped the handler rather than rejecting setup immediately.

The fix uses named setup callbacks and one cleanup function. Cleanup removes open/message/close/error callbacks before local closure or live Relay construction, so synchronous or delayed Close delivery cannot re-enter setup. A valid welcome replaces the setup listeners with the Relay's normal message/close listeners. Invalid welcomes keep the existing `1002` behavior; normal error and timeout messages remain unchanged.

Four small controlled-socket tests cover:

- Early error and Close followed by delayed open and welcome, with zero setup listeners or join sends remaining after failure.
- A one-millisecond join timeout with no peer Close event, releasing every setup listener and requesting socket closure.
- A synchronous join-send failure, preserving the exact error object and cleaning up immediately.
- A valid welcome, retaining BigInt IDs and exactly the live Relay listeners.

The first two fail against the original code in 43 ms. All 13 focused client tests pass after the fix in 71 ms. The complete 20-test client set then passes against disposable native and legacy-bridge processes in 1.09 seconds, including the existing one-second idle check. Both processes exit zero, and final connection/session/queued-byte/variable-byte gauges are zero. The standard npm test command includes the new tests, so `scripts/check-js.sh` also discovers them through npm.

The browser ES-module bundle rebuilds successfully, and JavaScript syntax and diff checks pass. No browser UI was executed for this change; the controlled EventTarget tests and Node WebSocket integration are the runtime evidence. No throughput or memory-use measurement is claimed: listener counts demonstrate setup ownership, not a heap profile.

`build.json` identifies the source base, exact client/test files and built browser bundle. The full selected client patch, raw failed/passing test logs, live fixture command/receipt/server logs and source files are retained. Run `python docs/benchmarks/2026-10-18-js-connect-cleanup/verify.py` for an offline checksum and receipt audit. The live receipt pins its frozen native and bridge executables. The base already includes the selected native shutdown-drain fix.

The browser bundle in `target/browser` is refreshed. No native or bridge executable or running service was replaced. The three pre-existing dirty source files remain byte-for-byte unchanged.
