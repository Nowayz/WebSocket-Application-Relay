# C++ no-op output-accounting experiment

**Decision: reject the runtime prototype; keep the accepted C++ executable and static library.**

The candidate skips atomic updates when buffered/released output bytes are unchanged. It changes no quotas, counter layout, payload ownership, queue shape or protocol rules. It is reconstructed from commit `8119669` plus the saved runtime patch. This intentionally isolates work that had previously been bundled with a rejected counter-layout experiment.

## Evidence

- Fresh diagnostic profile of the accepted build: `submit` 12.85%, `write` 12.26%, and `reconcile_buffer` 5.25% of sampled cycles. This is diagnostic sampling, not a timing comparison. Perf build IDs and the driver are preserved.
- Eight fresh rotated 8-second private trials with 32 pairs, four individually pinned server workers and the same frozen checked client; 1 KiB payloads, 16-frame windows.
- Four one-worker private guards and four four-worker fanout guards, each six seconds. No consistent CPU-efficiency advantage appears.
- Two independently named four-trial 100,000-user studies. Eight fresh C++ processes, 400,000 checked probes/s for 30 seconds, 120 samples per user: **96,000,000 timed probes verified**. Server CPU is captured between the ready/measured gates; setup, CSV sorting and teardown are excluded.
- All census CSV hashes, unique IDs, 120 samples/user, gate counts, final gauges and exact descriptor retirement are independently checked. The unchanged native corpus also passed 776 cases / 2,311 exchanges on the candidate.
- Ordinary timings are sequential; there is no concurrent build, profile or sanitizer workload. The new proxy-runner test is a separate integration smoke, not a performance result.

## 100,000-user results

| Study/pair | Control CPU-s | Candidate CPU-s | Candidate change | Control/Candidate p99 of user p99 (ms) | Control/Candidate worst user p99 (ms) |
|---|---:|---:|---:|---|---|
| initial/1 | 50.24 | 49.54 | -1.39% | 1.904 / 1.952 | 3.478 / 3.262 |
| initial/2 | 50.34 | 50.02 | -0.64% | 2.695 / 2.154 | 5.445 / 5.705 |
| confirmation/1 | 50.18 | 49.17 | -2.01% | 1.759 / 1.767 | 3.051 / 3.386 |
| confirmation/2 | 49.69 | 50.21 | +1.05% | 2.332 / 1.811 | 5.535 / 3.323 |

Total population CPU is 200.45 versus 198.94 CPU-seconds: a 0.75% aggregate reduction, with one matched pair reversing the result. This small difference and mixed latency tails do not justify adopting the extra fast-path branches.

## Placement/copy-path confounding

In the small-population private trials, the control used 1.6726 CPU µs per verified delivery and the candidate 1.6714: essentially unchanged. The diagnostic retained-copy proxy ranged from about 58.7% to 87.0% of deliveries across fresh processes. Fresh SO_REUSEPORT socket placement is a material source of workload variation; these runs do not isolate a topology-independent throughput effect.

The proxy is `war_copied_bytes_total / (1037 * verified_deliveries)` for this 1 KiB private workload. It counts retained copies, including any prefix fallback, local queued work and teardown notifications. It is an approximate copy-path diagnostic, not a census of worker ownership, and must not be compared directly with Rust routing-copy metrics. The final metrics themselves remain the original unmodified measurements.

Future `cpp/compare.py` traces record the retained-copy bytes per verified operation and this private-workload estimate, with the qualification included in the JSON. A real two-server smoke verified that the field is emitted for C++ without changing the tested runtime.

## Provenance and restoration

- Both frozen source trees and build records are under `source/`; runtime binaries are intentionally omitted from Git. The rejected patch and all candidate sources are retained.
- Control executable: `b9dc20b92e7c09f2188d1780da8d691730112f442e60447de62ee6b98ae86da1`.
- Rejected candidate executable: `2c28dcf0a92713febb8d8db235d30cff67901d0dda027c2e3ffabec717294a55`.
- The accepted source, executable, library and build metadata were restored from the frozen control and their source/binary hashes checked. The original C++/Rust comparison archive remains unchanged. No Rust runtime or manual service was replaced.
- Compressed CSV/large-JSON originals have separate uncompressed hashes in `manifest.json`.

Next performance work should control or explicitly characterize connection placement before interpreting small private-path changes, then address the measured cross-worker submission/retention cost. This study does not establish that no-op atomic instructions are free; it establishes that this particular guard change did not earn selection in the measured complete relay.

Verify this archive with `python3 docs/benchmarks/2026-10-17-cpp-zero-accounting/verify.py`.
