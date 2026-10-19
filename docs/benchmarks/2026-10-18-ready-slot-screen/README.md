# Cached ready-slot screen — 2026-10-18

Decision: unselected. Keep the current token-only ready queue. Carrying a slot hint removes a repeated token-map lookup, but the short CPU gain does not establish a useful full-population responsiveness/resource tradeoff.

The native scheduler already resolves a stable token to a slot when scheduling. The prototype stores both in each ready entry. Execution checks that the cached slot still has the same never-reused token; otherwise it resolves the canonical map after compaction. Socket readiness, FIFO and ownership remain unchanged. It adds no map, shared lock, body copy or token-generation packing. On this 64-bit host ready metadata changes from one to two machine words (8→16 bytes per slot); connection, Frame and Pending definitions are untouched.

All 81 clean library tests pass. Existing arena tests now verify that queued live tokens recover moved slots after compaction, retired tokens do not resolve, and an old ready entry cannot address a newly reused socket slot. The accept-continuation test still checks unique listener scheduling. Two focused arena tests pass separately. These assertions test the hint rather than bypassing it through the token map.

Four fresh two-second mixed fixtures use 64 private pairs, 2,048 fanout readers, window four, four pinned server and eight pinned client workers. Order: control/candidate/candidate/control. CPU-s per million verified deliveries is 1.306/1.284 for control and 1.276/1.268 for candidate. However, private observed maxima are higher in both candidate runs (5.63/6.75 ms versus 3.66/4.36). This unpaced mix is not a fixed-work causal comparison.

Two deciding six-second offered workloads use the same frozen client and options with 100,000 private users, 8,192 fanout readers, sixteen pinned client workers, eight server workers, 4 Hz private probes/churn, 16 Hz four-message fanout and two CBOR inspectors. Setup, quiescent proof scans and exhaustive retirement take additional wall time. No build/profile/other benchmark overlaps offered traffic.

| Metric | Control | Slot hint candidate |
| --- | ---: | ---: |
| Completed/offered fanout windows | 96/96 | 96/96 |
| Server CPU-seconds | 20.44 | 21.02 |
| Median private user maximum ms | 1.459 | 1.453 |
| Population p99 private maximum ms | 9.612 | 5.447 |
| Worst private maximum ms | 13.309 | 7.665 |
| Median recipient fanout maximum ms | 71.109 | 71.330 |
| Worst recipient fanout maximum ms | 81.334 | 80.112 |
| Worst intended-send fanout maximum ms | 92.179 | 103.688 |

Private tails improve against a control whose tails are worse than earlier independent controls; this single pair does not establish a repeatable fairness win. Native CPU rises about 2.8%, and intended-send fanout delay worsens about 12.5%. Send-relative fanout maximum is slightly lower. The larger queue entries and fallback logic have not earned selection from these mixed results. This is not proof of a universal regression or a claim that token-map lookups are free.

Every large-run private user has 24 samples (so per-user p99 equals maximum), every fanout recipient 384. Both have 2.4 million timed private observations and 3,145,728 fanout deliveries. All six fixtures pass checked payloads, FIFO, exact counters, normal exits and final resource gates. The large runs additionally verify all offers, affinity, population uniqueness, decoded CSV/timeline hashes and descriptor retirement. Finite client/kernel scheduling measurements do not isolate relay-hop latency or establish long-term SLAs.

The candidate patch applies to the clean base in builds.json, excluding three pre-existing dirty root overlays. The control is the latest frozen native build; intervening source commits are diagnostics/client/docs, not native behavior changes. Full per-user matrices, small receipts, test/build logs and executable identities are retained. Run python verify.py for offline validation. No selected scheduler, default executable, manual service or protected edit changed.
