# Notification load guard: short profile and rejected screen

Decision: retain the existing runtime. A read-before-exchange guard for an already pending notification shows a small fanout-only CPU benefit, but no consistent CPU benefit in mixed traffic. The latency results differ by workload. This evidence does not justify adoption.

A separate two-second current-relay fanout profile attributes 16.15% of sampled user cycles to `Connection::drive_batched`, 11.14% to `Socket::notify`, 9.01% to `write_batch`, and 8.96% to the vDSO clock. The notification annotation places 65.08% of local sample weight at the instruction after its atomic exchange and 34.47% after pending-queue lock operations. Sample attribution identifies a worthwhile experiment; it does not prove atomic contention or exact instruction cost. No samples were reported lost. Profiling was separate from all ordinary timing runs.

The isolated candidate adds a relaxed load before the existing relaxed atomic exchange. The exchange remains the claim operation when the loaded flag is false. Existing notify calls run under the socket state mutex; the owner resets the flag before acquiring that mutex to drive output. No notification queue, ownership, deadline, admission, payload or batching policy changes. Avoiding the read-modify-write when the flag is already true is plausible, but the common false case also gains a load and branch.

Eight ordinary runs measure two seconds each. Each workload uses control/candidate/candidate/control order, fresh processes, 2,048 readers, four 1 KiB byte-string messages per fanout window, four pinned native workers on CPUs 0-3, and eight client workers on CPUs 4-7,20-23. Mixed traffic adds 64 private pairs. Inspectors and churn are disabled. Builds, tests and profiles do not overlap ordinary traffic.

| Workload | Variant | CPU-seconds/million verified deliveries | Private maximum (ms) | Fanout maximum (ms) |
| --- | --- | ---: | ---: | ---: |
| fanout | control | 1.0486 | 0.000 | 7.754 |
| fanout | candidate | 1.0177 | 0.000 | 6.421 |
| fanout | candidate | 1.0412 | 0.000 | 9.231 |
| fanout | control | 1.0553 | 0.000 | 7.058 |
| mixed | control | 1.2663 | 6.416 | 20.293 |
| mixed | candidate | 1.2799 | 4.396 | 12.577 |
| mixed | candidate | 1.3502 | 3.404 | 12.181 |
| mixed | control | 1.3773 | 4.616 | 20.586 |

The mixed CPU ranges overlap substantially; candidate private/fanout maxima improve there, while fanout-only maxima are mixed. Unpaced traffic can change its private/fanout delivery mix, so CPU per million total deliveries is a screening statistic rather than fixed-mix service cost. This short result supports leaving the simple existing notification path alone; it establishes neither a universal regression nor an improvement. No 100,000-user or latency-SLA claim follows.

All eight ordinary receipts pass payload/FIFO, exact counter and final retirement checks, with clean process exits and zero recorded routing-copy bytes. All 90 clean candidate library tests pass in 0.51 seconds. The candidate build confirms recompilation; the control and candidate source manifests differ only in `src/server/socket.rs`. The patch is against clean source at c25da3c, excluding the three protected dirty files. Existing services and convenience binaries are unchanged.

Raw compressed receipts, source/binary build identities, profile report/annotation and tests are retained here. The full perf recording remains at local `artifacts/current-fanout-profile.perf.data`; its report is diagnostic and excluded from the ordinary comparisons. Run `python docs/benchmarks/2026-10-18-notification-load-screen/verify.py` to recompute and audit the ordinary evidence offline.
