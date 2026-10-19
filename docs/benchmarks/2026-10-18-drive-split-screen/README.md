# Output-only connection-drive split — 2026-10-18

Decision: leave the runtime unchanged. Splitting the output-only fast path from the complete connection state machine does not establish a repeatable whole-relay benefit across these short workloads. This is a rejected experiment, not a selected performance improvement.

The preceding [selected text profile](../2026-10-18-simd-text-validation/README.md) attributes 10.50% of native user self cycles to `Connection::drive_batched`. Its annotated release entry reserves 0xb88 bytes (2,952 bytes) of stack before checking the output-only branch. A stack reservation alone is not evidence that all those bytes are touched or that it is expensive. It motivated a narrow experiment rather than a performance claim.

The candidate retains the existing output-only checks in `drive_batched`, computes the same single timestamp at entry, then calls an outlined `drive_full` for all other work. It adds 13 source lines and changes no timeout, admission, FIFO, payload ownership or write quantum. In the candidate release build, the wrapper inlines into `Worker::run`; the large state-machine stack is confined to `drive_full` (0xb78 bytes). `Worker::run` grows from 8,319 to 9,136 text bytes, while the outlined state machine changes from 22,201 to 21,653. This shifts code layout and call boundaries; it does not remove the full state machine or its stack from fallback work.

Twelve ordinary relay runs each measured two seconds. For each workload the order was control/candidate/candidate/control, with fresh server and client processes, four native workers pinned to CPUs 0–3 and eight client workers pinned to CPUs 4–7,20–23. The frozen client was identical for every trial. Builds, tests, profiling and other timed traffic did not overlap the measurements. Setup and checked connection retirement add a few seconds outside the measured phase.

The mixed cases use 64 private pairs and 2,048 fanout readers, with four messages per fanout window. Byte and multibyte text values each have 1 KiB contents. The fanout-only case uses the same 2,048 readers, byte values and window, with zero private pairs. Membership churn and inspectors are disabled. These are unpaced workloads: delivery mix and total rate can vary. CPU per million deliveries is useful for screening but is not a fixed-mix service-cost measurement.

| Workload | Control CPU-s / million verified deliveries | Candidate | Control fanout maximum (ms) | Candidate (ms) |
| --- | --- | --- | --- | --- |
| Mixed byte strings | 1.326, 1.293 | 1.220, 1.263 | 14.427, 13.579 | 12.565, 16.995 |
| Mixed multibyte text | 1.410, 1.299 | 1.408, 1.373 | 18.440, 14.728 | 15.858, 15.904 |
| Fanout only | 1.162, 1.099 | 1.141, 1.127 | 18.997, 6.916 | 7.570, 7.228 |

The byte-string CPU benefit does not repeat in the text fixture. The fanout-only return control uses less CPU per delivery and has a lower maximum than either candidate. A simpler call path and one favorable pair of trials therefore do not justify adoption. No full-population test was run: the short screen already fails the selection criterion. These trials establish neither 100,000-user capacity nor a latency SLA.

All 12 trials pass payload/FIFO and exact counter checks, clean process exits, and final resource retirement, with zero copied routing payload bytes. All 83 clean-source library tests pass in 0.49 seconds, including output-only deferral for control/close/expired policy and partial-write FIFO/accounting tests. These checks support correctness of the experiment, not a performance win. No new runtime tests or production configuration were added.

`builds.json` pins the clean base and frozen executable hashes. `raw/candidate.patch.gz` reconstructs the isolated source against that base, excluding the three pre-existing dirty files. The archived native annotation comes from the earlier diagnostic and is not a new ordinary timing sample. Full ordinary receipts retain commands, rates, maxima, affinity, counter checks and exits. Run `python docs/benchmarks/2026-10-18-drive-split-screen/verify.py` to check the archive and recompute the table without starting any relay.

The selected native source remains at the borrowed SIMD text implementation. Manual native/bridge services, default executables and the three protected working-tree files are unchanged.
