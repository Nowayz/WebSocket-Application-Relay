# On-demand vectored-write headers: short rejected screen

Decision: retain the existing writer. Preparing headers while appending scatter/gather slices removes a temporary length array and avoids preparing headers beyond the byte quantum, but it loses this short whole-relay comparison. No runtime change was selected.

The candidate only changes `src/server/socket.rs` against clean source at 2692f3a. The frozen control source manifest differs only in that file. Both preserve the 16-frame/32-iovec and 256 KiB quantum limits, shared borrowed bodies, partial-write offsets, FIFO and admission. There is no new allocation or configuration. Source/assembly-level simplicity alone is insufficient to select it.

Four fresh-process runs measured two seconds each in control/candidate/candidate/control order. Each used 2,048 fanout readers, four 1 KiB byte-string messages per window, four native workers on CPUs 0-3 and eight client workers on CPUs 4-7,20-23. Private pairs, inspection and churn were disabled. Builds and tests did not overlap timed traffic. Setup and verified cleanup are outside the two-second workload.

| Variant | CPU-seconds per million deliveries | Deliveries/second | Observed maximum latency (ms) |
| --- | ---: | ---: | ---: |
| control | 1.0065 | 1,935,925 | 7.895 |
| candidate | 1.1480 | 1,716,164 | 10.174 |
| candidate | 1.1867 | 1,614,843 | 8.696 |
| control | 1.0921 | 1,823,246 | 7.532 |

Both candidate CPU costs exceed both controls; the candidate maxima also exceed both controls. This unpaced workload is a rejection screen, not a fixed-rate capacity test or causal diagnosis of the generated code. It does not establish performance for large bodies that exhaust the quantum, a 100,000-user population, or a latency SLA. The result is sufficient to avoid adopting this general writer change.

All four runs pass payload/FIFO and exact counter checks, clean exits, zero routing-copy counters and final resource retirement. All 90 clean candidate library tests pass in 0.36 seconds, including partial vectored-write FIFO/accounting checks. The frozen candidate build confirms actual recompilation. Existing manual services and the three protected working-tree overlays were preserved.

The raw build manifests identify all snapshot sources and binary hashes. The patch reconstructs the isolated experiment; raw ordinary receipts and the test log preserve the results. Run `python docs/benchmarks/2026-10-18-header-on-demand-screen/verify.py` to check the archive offline.
