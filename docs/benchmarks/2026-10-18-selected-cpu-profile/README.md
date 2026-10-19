# Selected native CPU attribution — 2026-10-18

Two sequential diagnostic profiles, each with two seconds of traffic, examine the selected single-entry writer binary. The binary is frozen at `artifacts/candidates/single-write-outline/war-relay`; full identity and commands are in the receipts. Both use 2,048 fanout recipients, four messages per window, four pinned server workers and eight pinned client workers. Mixed traffic adds 64 private pairs. Setup and retirement take additional time. No runtime change was selected from this profile alone.

| Symbol (self cycles) | Mixed | Fanout only |
| --- | ---: | ---: |
| Connection::drive_batched | 12.25% | 16.57% |
| Socket::notify | 4.69% | 12.12% |
| __vdso_clock_gettime | 4.59% | 9.98% |
| socket::write_batch | 3.92% | 9.36% |
| bytes_mut::shared_v_drop | 3.17% | 5.28% |
| Connection::deadline | 1.56% | 4.76% |

These are sampled self costs, not wall time, lock-wait time, inclusive costs or predicted optimization gains. Approximately 3,000 samples per profile and zero lost samples were reported. Sampling used `cycles:u` at 499 Hz with DWARF stacks. Some syscall trampoline samples appear with unresolved kernel addresses; this is not a complete kernel-cost profile. VDSO clock stacks did not unwind to their native callers, so this evidence cannot assign clock cost to a particular call site.

Both fixtures passed checked payloads, FIFO, exact counter and retirement gates and exited normally. Timings from these profiled runs are diagnostic; they are not compared with ordinary release timings and do not establish 100,000-user performance.

The comparison moves the next investigation toward fanout scheduling/notification and time acquisition rather than admission retry elimination. Prior duplicate-notification instrumentation found no repeated true notification flag in its measured workload, so simply adding a load before the notification swap is not justified. Any clock reuse experiment must retain write-timeout, idle-timeout, ping, reclaim and fairness behavior. Do not remove timestamps or weaken accounting based only on these percentages.

The full reports and compressed receipts are archived. Large original perf data remain at the absolute local paths in `profiles.json`, pinned by SHA-256. Verify portable evidence with `python verify.py`; original perf data are not required. No manual service was restarted and no protected workspace edit was changed.
