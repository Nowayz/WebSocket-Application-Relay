# Short fanout workload and observer diagnostics

Eight full-population trials use two seconds of traffic each (16 seconds total), with setup and checked cleanup outside traffic. The unchanged selected native relay, capacity client and inspection client are hash-pinned in every receipt. All processes exit zero, every offered window completes (32/32), and exact payload/FIFO/counters and final resource retirement pass. No relay runtime change or executable promotion is made.

Each trial holds 100,000 private users in 6,250 rooms of 16: four checked 1 KiB probes per user per second, eight observations per user (800,000 private observations/run). A separate room has 8,192 readers and a publisher sending four 1 KiB messages per window at 16 Hz: 128 observations per reader and 1,048,576 fanout deliveries/run. No inspectors, extra private pairs or fanout-audience membership changes. Eight native workers and each eight-worker client use the same disjoint physical CPU sets, with per-thread affinity checked. No TLS. Full commands and topology are in receipts.

First, churn isolation runs eight/zero/zero/eight independent lifecycle workers. Rate zero means unpaced when workers are enabled; it never meant disabled. The previous short-current report incorrectly described all churn as disabled; its prose is corrected and its raw results preserved. Zero-churn fanout tails remain substantial, so churn is not the sole explanation. Removing churn changes both server and client work, preventing a server-only attribution.

| Group/run | Churn workers | Timed FD scans | Server CPU-s | Private worst ms | Fanout worst ms | Monitor gap ms |
| --- | ---: | --- | ---: | ---: | ---: | --- |
| churn/0-w8 | 8 | True | 7.23 | 9.234 | 68.610 | 461.1–487.3 |
| churn/1-w0 | 0 | True | 6.72 | 5.608 | 80.547 | 453.0–506.6 |
| churn/2-w0 | 0 | True | 6.74 | 6.274 | 76.799 | 457.3–506.0 |
| churn/3-w8 | 8 | True | 7.44 | 6.729 | 90.593 | 480.0–507.8 |
| monitor/0-light | 0 | False | 6.24 | 3.850 | 50.514 | 251.0–251.3 |
| monitor/1-heavy | 0 | True | 6.69 | 8.506 | 58.971 | 454.4–513.8 |
| monitor/2-heavy | 0 | True | 6.69 | 10.063 | 61.888 | 452.3–499.2 |
| monitor/3-light | 0 | False | 6.55 | 4.462 | 56.170 | 250.9–251.2 |

Second, monitoring isolation runs light/heavy/heavy/light with zero churn throughout. Heavy performs the original full `/proc/PID/fd` enumeration every monitoring iteration; light reads CPU/RSS and records timed `fds: null`. Phase-gate FD checks, exact census, memory checks, affinity checks and complete final retirement remain enabled in both. Heavy samples count 108,236 descriptors. Lightweight monitoring improves both repeat ranges in this local screen, while reducing controller interference. These are different observer configurations around the same server, not a relay speedup. CPU is sampled at existing ready/measured gates including settling, not exactly two traffic seconds.

An independent calibration holds 100,000 `/dev/null` descriptors in a child. Full snapshots take 145–186 ms wall and 144–185 ms controller CPU; CPU/RSS-only snapshots take 0.029–0.215 ms. The child exits zero and releases all descriptors. This measures controller overhead, not network performance.

Existing controller timestamps now produce `fanout_window_timing` after measurement without adding clocks to the hot path. It separates wake lag, publisher work, last-send-to-all-ACK interval, membership cleanup and scheduled-to-complete time. The last-send interval includes server/kernel/reader/ACK scheduling. It cannot isolate server-hop latency. Earlier saved first windows are faster than later worst windows, so reader startup alone does not explain the tail. No reader scheduling or relay queue changes are justified by that observation.

These short runs expose specific costs and regressions; eight private observations per user cannot establish a long-term per-user p99 or latency SLA. The two-second repeats do not prove durability or Internet behavior. Finite per-user maxima and intended-send metrics remain in full CSVs for all users, not a sampled subset.

Six independent clock/FD tests pass in 0.001 seconds. Three real two-second small-population fixtures verify paced success, deliberate missed-load failure with correctness still true, and unpaced success; all explicitly disable churn and verify no lifecycle work. The integration suite takes 7.061 seconds including setup/cleanup. Artifact collision checks pass in 0.879 seconds. No timed comparison overlaps builds, other tests or compression.

Run `python docs/benchmarks/2026-10-18-fanout-workload-diagnostics/verify.py` for offline verification. `raw/` contains complete receipts, full per-user CSVs, window timelines, logs, both runner variants, tests, orchestration, calibration and the tracked patch against fba2e8d; the new standalone timing test is also archived. Binaries are identified by SHA-256 in every receipt. Existing manual services and the three unrelated dirty source overlays are untouched.
