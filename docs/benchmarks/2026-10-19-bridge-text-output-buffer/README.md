# Reserve the complete legacy text output

CBOR-to-legacy text conversion previously allocated the 12-byte base64 sender prefix, then grew that string to append the payload. It now reserves `12 + payload.len()` once and uses the existing base64 `encode_string` API to append the prefix before the payload. The fixed prefix is the base64 representation of an eight-byte sender ID. Production unsafe-code restrictions remain unchanged; the implementation adds no unsafe code, pooling, shared state, or new dependencies.

## Short allocation experiment

Fresh control and candidate bridge processes each handled 256 distinct joined clients and eight sequential text messages per client. Each payload contained an eight-byte ASCII sequence followed by 1,016 bytes of UTF-8 text. An independent masked WebSocket client checked opcode, exact sender/payload bytes and FIFO on all 2,048 messages per variant, then completed clean Close handshakes. The native relay reported no routing payload copies or error counters.

External tcmalloc profiling captured baseline, joined, warmed and retired checkpoints. Live requested heap and live object counts matched exactly at all four checkpoints. Relative to the joined checkpoint, candidate traffic performed **2,048 fewer allocations** and requested **24,576 fewer bytes**, exactly one allocation and 12 bytes per message. This removes the separate prefix allocation and its growth; the final output still owns a copied payload. Absolute cumulative bytes differ by one additional byte already present at baseline, which is excluded. Descriptor counts returned to baseline. Profiling is allocation evidence, not ordinary RSS or latency evidence.

## Conversion timing and correctness

`conversion-probe.rs` calls the complete public `from_cbor` conversion, checks the exact output including UTF-8, and consumes each result through `black_box`. Input packets are prepared outside timing. Four separate release processes ran control/candidate/candidate/control with payload sizes 0, 1, 12, 64, 1,024 and 65,536 bytes. Each size used 100,000 conversions except the largest, which used 10,000.

| Payload bytes | Control ns/conversion | Candidate ns/conversion |
| --- | --- | --- |
| 0 | 37.6–46.3 | 40.7–49.7 |
| 1 | 48.4–55.4 | 40.7–48.2 |
| 12 | 49.3–49.8 | 47.4–48.0 |
| 64 | 74.1–75.5 | 65.2–68.6 |
| 1,024 | 427.4–437.4 | 400.4–412.4 |
| 65,536 | 21,687.6–21,892.2 | 21,854.9–22,107.3 |

The mid-sized cases improved in this probe. Empty messages were noisy and the largest case was effectively unchanged. These short conversion measurements do not establish end-to-end latency, throughput, fairness or 100,000-user performance. The change is retained for the directly measured per-message allocation reduction and simple ownership.

The first diagnostic harness attempted an in-process counting allocator and was rejected by the repository's unsafe-code lint. That harness was removed without changing the lint; allocation measurements use the external profiler instead. The failed build receipt remains in `artifacts/candidates/bridge-text-buffer-control`. The successful control example is `bridge-text-buffer-control-v2`; the example candidate is `bridge-text-buffer-candidate`; the socket candidate is `bridge-text-buffer-server`. Each directory under `artifacts/candidates` contains its frozen build manifest. The socket control is `bridge-text-join-candidate/war-legacy-bridge`.

## Verification

All 106 Rust library tests passed in 0.54 seconds, and Clippy passed with warnings denied. All 61 JavaScript socket integration tests passed in 1.112 seconds; all six checked native resource gauges returned to zero and both fixture processes exited successfully. Existing manual services and unrelated worktree changes were preserved.

Receipts and exact heap totals are alongside this report. Raw capture paths and hashes are recorded in the receipts. Local helpers are `artifacts/bridge-text-buffer-memory-probe.py`, `artifacts/bridge-text-buffer-heap.py`, and `artifacts/analyze-bridge-text-buffer.py`.
