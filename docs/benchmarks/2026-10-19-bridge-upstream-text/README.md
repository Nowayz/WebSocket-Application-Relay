# Clean legacy Close for unexpected upstream text

The bridge requires binary CBOR messages from its upstream native relay. After a valid welcome, an upstream text frame previously fell through to loop termination and dropped the legacy TCP connection without a WebSocket Close. A faulting-upstream socket test reproduces that unclean disconnect on the original code.

The bridge now sends the legacy peer Close **1002 / Invalid Upstream Message** through its existing bounded close helper before retiring the connection. This affects only the invalid upstream text branch. Binary forwarding, payload ownership, FIFO, queue limits, and ordinary upstream Ping/Pong/Close handling are unchanged. No new runtime state, lock, allocation strategy or dependency is added. This does not add a clean Close handshake with the faulty upstream; it protects the legacy client from the previous unexplained raw disconnect.

The new regression uses a one-slot bridge and a controlled upstream. It sends a valid welcome followed by unexpected text, verifies the exact frontend Close code/reason, acknowledges the Close and verifies EOF. It repeats through the same single slot to prove retirement and reuse. The fixture has a two-second overall deadline and uses no sleeps. The original test fails in 0.04 seconds with the recorded unclean-disconnect assertion.

All 107 Rust library tests pass in 0.54 seconds; Clippy passes with warnings denied. The freshly frozen bridge also passes all 61 JavaScript/native/legacy socket cases in 1.108 seconds against the selected native fixture. All six checked native resource gauges return to zero and both fixture processes exit zero. This is protocol and cleanup validation, with no performance or mass-capacity claim.

The reproduction, test and Clippy logs, fresh build manifest and integration receipt are retained alongside this report. The frozen executable is `artifacts/candidates/bridge-upstream-text-selected/war-legacy-bridge`. No existing manual service was restarted and unrelated source changes were preserved.
