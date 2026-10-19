# Inline large integer reply screen — 2026-10-18

Decision: unselected. Retain the ten-byte Frame prefix and existing large-integer reply fallback. Using four padding bytes to remove a cold allocation looked attractive, but did not establish a worthwhile ordinary-workload tradeoff.

Two isolated candidates enlarge the prefix from 10 to 14 bytes and encode all reply integers inline. Frame remains 48 bytes on this 64-bit host. The first retains the two-pass prefix/value encoding; the second uses one encoder chain. Large replies no longer carry a heap-backed value body. No routing body is copied by the change. This does not claim the same object size on 32-bit platforms.

Construction microbenchmarks use seven batches of 200,000 operations per process, CPU 4, with sequential control/candidate/candidate/control ordering. The frame source is included by the benchmark, so it measures the actual private constructor. It is not socket throughput. Clone cases create/drop eight clones sequentially, not concurrent output owners.

The original two-pass inline variant increases unshared construction to roughly 10–11 ns from 6–7 ns. The one-pass variant reduces this to about 8 ns but still trails control. For a 64-bit value, eight-clone medians drop from 178–209 ns to about 85 ns with the one-pass variant. The returning control clone runs include a substantial excursion, retained in full; the initial comparison also shows roughly 175–178 ns versus 82–89. No allocator-call counter was used, and compiler allocation elision may affect construction timing. Payload emptiness and object size are measured separately.

The one-pass variant then ran four ordinary two-second mixed fixtures, with 64 private pairs, 2,048 fanout readers, four messages/window, four pinned server and eight pinned client workers:

| Order | Variant | CPU-seconds per million checked deliveries | Private maximum ms | Fanout maximum ms |
| --- | --- | ---: | ---: | ---: |
| 1 | Control | 1.2341 | 7.053 | 17.022 |
| 2 | One-pass inline | 1.3214 | 4.382 | 15.869 |
| 3 | One-pass inline | 1.2680 | 2.754 | 14.078 |
| 4 | Control | 1.2549 | 5.759 | 19.882 |

Both candidate runs use more CPU per checked delivery; observed maxima are lower. This unpaced screen changes the traffic mix with scheduling and does not establish a per-user fairness improvement under fixed offered load. The common-path cost is not justified by improved cloning of uncommon large-ID assignment replies. No population-scale latency claim follows and no hot-path gain is selected. Setup and checked retirement are additional to the two-second phases. No build/profile/other benchmark overlaps timed traffic.

All four mixed fixtures pass payload, FIFO, exact counters, normal exits and zero final live reservations/output queue capacity. Five isolated candidate frame tests pass, including all integer widths/opcodes and known CBOR vectors. These frame tests validate application bytes; the mixed fixture does not independently exercise large-ID assignment over a real WebSocket. No further integration was warranted after the negative screen.

Patches apply to the clean base in summary.json; pre-existing root overlays were excluded. Native control uses the previously frozen helper-sizing selected binary, whose runtime code matches this base (intervening protocol changes are documentation). Executable hashes, complete microbenchmark samples, full mixed receipts, source and successful build/test logs are retained. Run python verify.py for offline verification. Root Frame source, manual services and protected user edits are unchanged.
