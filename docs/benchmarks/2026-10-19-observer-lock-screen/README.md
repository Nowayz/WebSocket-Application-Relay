# Observer-index lock split: mechanism verified, performance unselected

This experiment evaluates the existing uncommitted Rust prototype without changing or committing either of its source files. The prototype moves the three authenticated observer indexes behind separate read/write locks, with counts retained under the membership registry. It releases the membership registry before channel/private observer output while retaining an index read guard to preserve permission/FIFO barriers. Ordinary no-observer private and room paths retain their fast branches. Global broadcast and departure handling are not claimed to have been redesigned.

The purpose is to reduce interference between observer output and unrelated joins/departures. This adds lock-order and count/index synchronization obligations, so it requires a demonstrated benefit before selection. The native runtime remains the committed implementation. The pre-existing C++ overlay is excluded from both builds.

## Source and mechanism checks

Fresh candidate and control releases were compiled sequentially in the same clean snapshot/target paths. Frozen source manifests differ **only in `src/relay.rs` and `src/relay/listener_tests.rs`**. The full existing prototype patch is archived here for replay against the recorded base. The original two Rust files and unrelated C++ file retain their original content hashes.

The candidate passes all **111 library tests in 0.54 seconds** and Clippy with warnings denied. Its existing tests check channel/private observer output without the registry held, retained body ownership, and permission-acknowledgement barriers while unrelated membership progresses. The implementation-independent channel-output check fails against the control with `observer output must release the membership registry`, proving this narrower lock-release difference. That proof does not itself establish better latency. An initial negative-test setup included layout-specific assertions and could not compile against the old layout; the final counterfactual retains only the compatible lock-release tests.

## Short fixed-rate workloads

Each scale uses control/candidate/candidate/control order, fresh fixtures and **two seconds of measured traffic per run**, plus setup and exhaustive retirement. Both scales use eight pinned native workers and separate pinned eight-worker private/fanout clients. All use 1 KiB values, four private probes/user/second, four fanout messages/window, 16 fanout windows/second and eight churn workers offering 16 lifecycles/second each. Every run completes all **256 churn lifecycles**. There is no timed inspection workload or extra unpaced private pair. Server heartbeats, application idle expiry and input rate limits are disabled; the ordinary message, queue and read-buffer bounds remain. No build, profiler or other benchmark overlaps timed traffic.

The small screen holds 4,096 private users and 2,048 authenticated global channel observers.

| Run | CPU seconds | 99th percentile of users' worst private ms | Worst private ms | Worst fanout ms |
| --- | --- | --- | --- | --- |
| Control 1 | 0.64 | 1.769 | 4.126 | 5.936 |
| Candidate 1 | 0.64 | 1.433 | 2.415 | 5.026 |
| Candidate 2 | 0.63 | 1.378 | 2.288 | 5.461 |
| Control 2 | 0.62 | 1.579 | 2.830 | 5.037 |

The lower candidate private tails justify checking the real target scale, but CPU/fanout ranges do not establish an improvement.

The full-scale check holds 100,000 private users plus 8,192 observers, with 108,194 native connections at readiness before timed churn. Each run verifies 800,000 measured private observations and 1,048,576 measured fanout observations, in addition to the churn checks.

| Run | CPU seconds | 99th percentile of users' worst private ms | Worst private ms | Worst fanout ms |
| --- | --- | --- | --- | --- |
| Control 1 | 6.13 | 13.358 | 17.242 | 32.053 |
| Candidate 1 | 6.50 | 14.926 | 18.194 | 35.440 |
| Candidate 2 | 6.45 | 15.261 | 19.782 | 33.535 |
| Control 2 | 6.52 | 15.608 | 18.337 | 35.726 |

At target scale, CPU and latency distributions overlap; the candidate does not repeat the small-screen tail advantage. These observations establish neither a universal regression nor an equivalence guarantee. They provide no basis to adopt the extra synchronization state. The existing prototype is left unselected and uncommitted. A proven lock-release property is kept distinct from a measured user-facing performance gain.

Every run meets the offered fanout rate and passes exact body/FIFO, counter, census, error and retirement gates, with all three disposable processes exiting zero. Checked connection/session, queue, variable, read-reservation and output-capacity gauges retire; all full-scale native slot capacity retires. The runner also verifies descriptors return to baseline. Private users have only eight observations each, so their empirical p99 equals their observed maximum. No long-term p99, latency SLA, real-NIC, TLS or soak-test claim follows.

There are sixteen seconds of measured traffic across all eight runs; setup and complete retirement add time. Existing manual services were not changed. Receipts retain commands, binaries, affinity, exact latency summaries and raw CSV identities. Raw CSVs remain local under their recorded artifact prefixes. Run `python verify.py` to audit the archived results and source differences.
