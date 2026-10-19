# Keep identical native deadlines in place — 2026-10-18

Selected: `Timers::set` returns immediately when an existing live timer already has exactly the requested `Instant`. Changed deadlines, cancellation, insertion, pop order and slot rekeying retain the existing implementation. No deadline rounding, cached timestamp, extra state, lock or allocation was added. This avoids heap removal/reinsertion and index repair for identical deadlines; the original remove/reinsert path did not normally allocate either.

A separate two-second census with 8,192 users and the normal 15-second keepalive interval records 150,845 identical-deadline updates among 178,207 total setter calls (84.65%). Those counters cover the whole process, including setup, traffic, postchecks and retirement, rather than only the measured phase. All setter calls in this fixture have a deadline. The diagnostic changes layout/instructions and prints per-instance counters at destruction; its CPU/latency values are excluded from optimization comparisons.

The first ordinary screen uses four native workers, eight pinned client workers, 8,192 private users, 16-user rooms, four probes/user/second and a two-second client phase. Four sequential processes in control/candidate/candidate/control order are measured with keepalive 15, then four with keepalive disabled. Native CPU is 0.28/0.28 CPU-s versus 0.29/0.27 with keepalive enabled, and 0.26/0.26 versus 0.26/0.27 disabled. These small cases do not establish a gain at the host's CPU-accounting resolution.

Because timer heaps grow with population, the same two-second private workload is also measured at the actual 100,000-user target, with eight native workers and sixteen client workers on disjoint individually pinned CPU sets. Every run verifies 800,000 deliveries and eight observations for every user, followed by routing postchecks and checked retirement. The first order is candidate/control/control/candidate, and a final-source confirmation runs control/selected after strengthening only cfg(test) duplicate-update coverage and rebuilding.

| 100,000-user run | Native CPU-s | Native user CPU-s | Whole-population p99 histogram upper bound | Worst observed packet |
| --- | --- | --- | --- | --- |
| Initial candidate 1 | 3.43 | 0.67 | 1.049 ms | 4.167 ms |
| Control 1 | 3.52 | 0.76 | 1.049 ms | 4.145 ms |
| Control 2 | 3.49 | 0.76 | 1.049 ms | 4.179 ms |
| Initial candidate 2 | 3.45 | 0.74 | 1.049 ms | 3.498 ms |
| Final control | 3.53 | 0.76 | 1.049 ms | 4.249 ms |
| Final selected | 3.44 | 0.71 | 1.049 ms | 4.029 ms |

The original candidate range is 3.43–3.45 versus control 3.49–3.52 CPU-s, roughly a 2% total CPU reduction. The final frozen build confirms 3.53→3.44 CPU-s in its matched pair. This is a modest result for keepalive-enabled private traffic, not a universal throughput win. CPU is the runner's ready-to-measured gate delta (gate wall times about 3.2–3.4 seconds), which includes keepalive/controller intervals outside the client's two-second data phase. It is not isolated packet-processing time. All runs have the same delivered count and essentially the same paced rate, about 398.5–398.7 thousand deliveries/second; there is no offered-throughput increase claim. Eight observations per user cannot establish a long-term tail guarantee.

Four additional two-second high-rate mixed trials with keepalive disabled use 64 private pairs, 2,048 fanout readers and window four. Native CPU per million verified deliveries is 1.332/1.375 for control versus 1.318/1.333 for the candidate. Maxima vary: private 3.760/4.464 versus 3.380/5.174 ms, fanout 15.221/13.772 versus 13.721/17.988 ms. These variable-mix results show no consistent CPU regression but do not establish a latency improvement. They are not a 100k fanout acceptance test.

All ordinary trials pass exact counters, payload/FIFO checks, clean exits and final resource retirement with zero copied routing payload bytes. All 92 working-tree library tests pass in 0.53 seconds; the ordering/reference-model test now repeats identical updates and cancellations to guard against duplicates or stale entries. Strict all-target Clippy and formatting pass. Both benchmarked and final-source builds pass the independent socket corpus (776 cases / 2,311 exchanges each). Only the final source's extra test statements differ from the first candidate source; both native binary hashes and the final confirmation are recorded.

## Build-cache issue caught before timing

The untouched control archive initially produced a Cargo `Finished` result in 0.03 seconds while reusing the diagnostic binary. The copied control hash equaled the census hash and contained `timer-census:`. No ordinary trial used it. Refreshing Rust source timestamps in the control snapshot's `src`, `examples` and `vendor` directories forced recompilation; the corrected control has a different hash and no diagnostic marker. Ordinary receipts all pin that corrected hash and contain no census output. The short stale-build log and detection receipt are retained.

When reusing a Cargo target directory across archived source snapshots, do not treat Cargo success or an executable's path as proof that it was rebuilt. Archive timestamps can be older than cached outputs. Force source freshness or use a clean build, inspect the compile log, freeze the result, and verify its hash/diagnostic identity before measuring. Keep builds sequential. This incident does not establish that unrelated earlier comparisons were stale; their own build/identity evidence remains necessary.

`builds.json` pins the clean base, diagnostic, corrected control, original candidate and final selected binaries, final timer source and CSV hashes. Full receipts, exact per-user CSVs/timelines, logs, runner sources, census patch and selected patch are retained. Run `python docs/benchmarks/2026-10-18-unchanged-timers/verify.py` to audit counts, source/binary identities, repeated results and retirement offline. It starts no services.

No manual service or convenience executable was replaced. The three pre-existing dirty source files remain byte-for-byte unchanged.
