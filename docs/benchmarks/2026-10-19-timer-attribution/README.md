# Real-connection timer attribution — 2026-10-19

The current equal-deadline fast path handles most timer setter calls with heartbeats enabled. With 100,000 private users and a 15-second heartbeat interval, 83.38% of calls retained the same live deadline; 9.16% changed an existing deadline, and 7.46% inserted a timer. This supports leaving the preceding [in-place update candidate](../2026-10-19-timer-key-update/README.md) unselected. It does not establish timer CPU cost or predict a packet-latency improvement.

A clean snapshot of committed source adds six local counters per timer heap and prints them at destruction: absent-to-absent, live-to-absent, insertion, unchanged live deadline, earlier live deadline, and later live deadline. It also records the peak heap length. There are no added locks or per-call allocations. The counters and extra reads alter the executed code; all runs are **diagnostics**, including when the generic runner's `profile_diagnostic` field is false because no perf process was requested. Do not mix their CPU or latency numbers into ordinary performance comparisons.

| Private users | Heartbeat interval seconds | Setter calls | Unchanged live | Changed live | Inserted | No deadline requested |
| --- | --- | --- | --- | --- | --- | --- |
| 8,192 | 0 | 171,747 | 1.59% | 4.76% | 4.77% | 88.88% |
| 8,192 | 15 | 174,444 | 84.46% | 9.39% | 6.15% | 0.00% |
| 8,192 | 1 | 214,965 | 77.59% | 7.62% | 14.79% | 0.00% |
| 100,000 | 15 | 2,182,965 | 83.38% | 9.16% | 7.46% | 0.00% |

Counts cover the complete process lifetime: connection setup, two measured traffic seconds, routing postchecks, and retirement. They are not phase-local counts. An insertion includes both a new timer and reinsertion after a timer has already been popped. The absent-to-absent category performs no heap repair; live-to-absent cancels an existing timer. Earlier/later values are exact Instant comparisons, with no deadline rounding. This census does not measure idle-timeout-enabled traffic, fanout, WAN/TLS behavior or an indefinitely connected population.

Each user sends four paced 1 KiB private probes per second in 16-member rooms. All four runs pass exact sender/body/FIFO, routing postchecks and counter/resource checks. The three 8,192-user runs each verify 65,536 measured deliveries; the 100,000-user run verifies 800,000. Rate limits are disabled and message, output-queue and read-buffer bounds remain enabled. The server uses four workers on CPUs 0–3 for small cases and eight on 0–7 at target population; eight client workers are individually pinned on 8–15. Every recorded affinity is verified after measurement. Full commands and executable hashes are retained in receipts.

All server/client processes exited zero. File descriptors returned to baseline; final connections, sessions, queued bytes, variable bytes, read-buffer reservations and output queue capacity were zero. Native routing-copy counters remained zero. Every diagnostic timer instance retired with an empty heap. This is bounded-load correctness evidence, not a long-term responsiveness guarantee. Existing manual services and the three unrelated worktree edits were preserved.

There are **eight measured traffic seconds total**, plus setup and exhaustive postchecks/retirement. No multi-hour tests were used. A separate deterministic test visits each of the six counter categories exactly once; it and the existing two timer ordering/rekeying tests passed within the runner's 0.01-second reporting resolution. `tested-timers.rs` is the diagnostic source followed only by that extra cfg(test) module.

The selected production timer source remains unchanged. `build.json` proves a non-cached release build from the copied diagnostic inputs. `provenance.json` pins sources, runner snapshots, compiler, receipts and build manifest. Compressed receipts preserve raw server counters and all generic-runner checks. Run `python docs/benchmarks/2026-10-19-timer-attribution/verify.py` to rederive every percentage and audit identities and retirement. `run.py` preserves the bounded sequential controller, referring to the original local frozen binaries; source and build commands are recorded for reconstruction in fresh paths.
