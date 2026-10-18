# Legacy bridge upstream read buffer, 2026-10-18

Code revision `1a3e494fc363aabb20804c267d41224db8f663c7` sets only the optional legacy bridge's initial upstream WebSocket read buffer to 4 KiB, matching its frontend. Previously the upstream used tungstenite's 128 KiB default. Nagle remains disabled. Write buffering, frame/message limits, protocol conversion, native routing, and connection limits are unchanged. No new pool, dependency, or scheduler is introduced.

The measured bottleneck was connection memory. A default upstream read buffer is allocated per connection and touched while reading the welcome. A smaller initial allocation avoids retaining that large slab for small-packet sessions. The decoder still grows to accommodate larger frames.

## Short population test

All runs use fresh disposable native and bridge processes on ephemeral loopback ports, four workers each, 256 unique joined legacy rooms, and 144 sequential 1 KiB self echoes per client. Eight Python threads drive traffic; each client has only one request outstanding. Every one of the 36,864 responses is checked for exact sender, payload, sequence, and FIFO. This is a memory/compatibility test, not a throughput benchmark or fairness distribution.

| Bridge anonymous RSS (KiB) | Same-path rebuilt control | Final candidate |
| --- | ---: | ---: |
| Before connections | 456 | 460 |
| 256 joined, after welcomes | 37,092 | 4,284 |
| After 36,864 checked echoes | 39,728 | 6,952 |
| After all clients retire | 3,764 | 4,452 |

Joined anonymous RSS falls by 32,808 KiB, approximately 128 KiB per connection and 88.5% of the control's total joined anonymous RSS. Earlier control/candidate/candidate/control runs reproduce the same roughly 32 MiB saving: joined RSS 37,096 / 4,320 / 4,324 / 37,092 KiB. Warmed RSS likewise falls by roughly 32 MiB. Retired allocator retention is slightly higher in the candidate; released connections do not imply immediate return of all allocator pages to the OS.

RSS excludes native relay, generator, kernel socket memory, and reserved but untouched virtual pages. These results do not establish 100,000-user capacity, WAN latency, fanout throughput, or native relay speed. The bridge's default connection cap remains unchanged. Do not extrapolate total-system capacity from this per-connection saving.

The original baseline receipt deliberately remains `passed: false`: all packet checks passed, but a harness assertion incorrectly included joins in the incoming application-message counter. Observed counter semantics are incoming = echoes, deliveries = welcomes + echoes. The corrected harness checks these exact counts; no server behavior was changed to satisfy the test.

## Compatibility and local acceptance

- Clean isolated library suite: 78 passed, 0 failed, test execution 0.56 seconds.
- Strict Clippy for the clean library and bridge passed; changed source passes Rust 2024 rustfmt check.
- Actual JavaScript native/legacy suite: 7 passed, none skipped, approximately 1.09 seconds including the one-second idle-policy test.
- Final candidate: 256 connections, 36,864 verified echoes, normal 1000 Close/EOF, zero routing copied bytes, zero error/pressure/rejection counters, all live resource gauges retired to zero, both processes exited successfully.
- Independent upstream fixture verified exact large payloads of 32 KiB, 128 KiB, 1 MiB, and 16,777,201 bytes; the final CBOR envelope is exactly the existing 16 MiB upstream frame boundary. These single-pass timings are compatibility evidence, not a statistically supported large-frame performance comparison. Smaller reads can require more syscalls for large frames; that remains a workload-dependent tradeoff.
- Delayed-ACK regression: all eight pairs passed, preserving the upstream TCP_NODELAY improvement. This controlled fixture is not a whole-server latency claim.
- Guarded local adoption replaced idle bridge PID 1055440 with 1064922. Native PID 966632 and static UI PID 219427 stayed running. Private delivery, broadcast, departure, two exact Close/EOF exchanges, running executable identity, and zero final resource gauges were verified. The convenience release bridge binary was aligned atomically. No hosted deployment was performed.

Selected executable SHA-256: `084fad16645b9f751678cbb3bbb7dcc20977ee196686dde52b7744894b30c5fb`. Build provenance is in `build.json`. All builds excluded the three pre-existing user overlays listed in `preserved-user-changes.json`; their working-tree hashes were checked unchanged.

## Reproduction and evidence

Run `python3 docs/benchmarks/2026-10-18-bridge-read-buffer/verify.py` for offline receipt/hash verification. The archive includes the probes and wire-parser dependency snapshots. To rerun, copy the probe scripts into the repository's `artifacts/` directory (their path resolution expects that location), build clean revisions with `git archive`, and supply explicit `--server`, `--bridge`, and fresh `--output` paths. The memory probe accepts `--clients` and `--rounds`; the defaults reproduce this study. The large-frame probe requires only `--bridge` and `--output`. Python probes use the standard library and the checked-in independent reference wire client; no downloads or system tuning are required.

The final control was rebuilt in the same clean source path and Cargo release target as the candidate. The candidate source differs only by the committed read-buffer configuration. Earlier candidate measurements used an equivalent inline configuration expression before the final local binding/comment; final receipts pin the actual adopted executable separately. Runtime worker counts, protocol limits, native binary, connection population, and payload sequence were held fixed.
