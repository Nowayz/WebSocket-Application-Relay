# Borrowed SIMD text validation — 2026-10-18

Selected: validate longer definite CBOR text with `simdutf8::compat::from_utf8`, retaining minicbor for tiny canonical strings and all other value types. This follows the [text-workload profile](../2026-10-18-value-kind-workloads/README.md), where standard UTF-8 validation accounted for 42.68% of native user cycles. No scheduler, admission budget, queue, locking or FIFO changes are involved.

The implementation copies at most nine CBOR header bytes into stack scratch, translates the major type to an unsigned argument, and reuses minicbor's length decoder. Checked length conversion, addition and input slicing precede validation. The text body remains borrowed from the original input packet; it is neither allocated nor copied. The enclosing iterative depth/item limits and rejection of indefinite values remain intact. The [simdutf8 compatible API](https://docs.rs/simdutf8/0.1.5/simdutf8/) provides runtime CPU selection and early failure on invalid input. The new dependency is version 0.1.5 in the lockfile. Join/control parsing that uses minicbor directly is unchanged.

Twelve ordinary relay trials each measured two seconds, sequentially in control/candidate/candidate/control order for each value kind. Each used 64 private pairs, 2,048 fanout readers, window four, 1 KiB value contents, four native workers and eight fixed client workers; server and client CPU sets were disjoint. Neither builds nor profiling overlapped traffic. The frozen client was identical throughout. Each cell below is the range of the two process results, not pooled samples.

| Value kind | Control native CPU-s / million verified deliveries | Selected | Control deliveries/s | Selected deliveries/s |
| --- | --- | --- | --- | --- |
| Multibyte text | 1.549–1.675 | 1.286–1.301 | 1.670–1.789 million | 1.997–2.011 million |
| Byte string | 1.265–1.435 | 1.318–1.334 | 1.783–2.049 million | 1.928–1.955 million |
| ASCII text | 1.301–1.402 | 1.245–1.336 | 1.830–2.004 million | 1.914–2.075 million |

Multibyte text shows a repeatable benefit in this fixture: about 20% lower mean CPU per delivery across the two process results. Private/fanout proportions vary in this unpaced workload, so this is not a fixed-mix service-cost estimate. Byte-string and ASCII ranges overlap; neither establishes a repeatable benefit or regression. Per-class rates and maxima are retained in `summary.json`. Short multibyte fanout maxima improve from 17.34–18.33 ms to 11.66–13.08 ms, but private maxima overlap and these few trials do not establish a latency guarantee.

A separate two-second candidate profile records 3,859 user-cycle samples with zero lost samples. The AVX2 compatible UTF-8 validator accounts for 9.51% of self cycles, versus 42.68% in the earlier control text profile. Candidate connection driving is 10.50%, socket send 8.61%, bounded relay receive 7.14% and bounded WebSocket receive 6.07%. These are diagnostic shares from separate variable-mix runs, not an absolute cycle-saving calculation. The profile's timings are excluded from all ordinary comparisons. Its raw perf data and symbol report are retained.

Whole-envelope parse microbenchmarks use seven batches per case and two independent processes per implementation in the same ABBA order, pinned to CPU 4. Input construction is outside timing; results are consumed through `black_box`. The 1 KiB multibyte case falls from 331.58–343.11 ns to 56.62–57.24 ns, roughly sixfold. At 64 KiB it falls from 19.873–19.884 microseconds to 2.667–2.671 microseconds. Small cases are not uniformly faster: 16-byte multibyte text increases from 18.14–19.72 ns to 20.17–20.40 ns, and 65-byte ASCII increases from 18.35–19.00 ns to 22.61–22.70 ns. All sizes and batches are retained; selection rests on the measured relay hotspot rather than a universal parser improvement claim. No custom CPU dispatcher or size-tuning table was introduced.

Two additional six-second byte-string capacity runs each exercised 100,000 private users plus 8,192 fanout readers, with 4 Hz private probes, churn and 16 Hz fanout windows. Both completed all 96/96 offered windows, 2.4 million private observations and 3,145,728 checked fanout deliveries, with clean process exits and zero final connection/session/queue/read-buffer/output-capacity gauges. Routing payload copied bytes remained zero. This checks the existing byte path at full population; it does not measure 100,000 private text senders.

| Full-population observation | Control | Selected |
| --- | --- | --- |
| Native CPU seconds | 20.77 | 20.50 |
| Median of per-private-user observed maxima | 1.429 ms | 1.497 ms |
| p99 of per-private-user observed maxima | 16.892 ms | 6.590 ms |
| Worst private observation | 21.569 ms | 11.712 ms |
| Worst fanout observation | 69.736 ms | 61.757 ms |

The candidate ran first here, then control. These single large trials show CPU neutrality and successful offered-load completion; the variable tails do not establish a fairness improvement. Each private user has only 24 observations, so its sample p99 equals its observed maximum. These finite measurements are not a long-term SLA, nor a C++ comparison.

Correctness validation includes 86 working-tree library tests (83 in the clean candidate), strict all-target Clippy, formatting, both release binaries, 16 JavaScript/native/legacy-bridge compatibility tests, and the independent socket corpus of 776 cases / 2,311 exchanges. Differential tests compare minicbor acceptance and successful decoder positions across every two-byte combination, UTF-8 boundaries, noncanonical length widths, truncated headers/bodies, overflow, and subsequent values. Pointer identity verifies that the returned encoded value still borrows the original packet. Invalid values still cause the protocol disconnect; decoder position after a failed validation is not promised.

`builds.json` pins frozen binaries and the clean base. `raw/selected.patch.gz` applies the selected source without the three pre-existing working-tree overlays. Raw receipts, full population CSVs and windows, component batches, commands, source and logs are losslessly compressed; the verifier checks decoded CSV hashes against the original receipts and recomputes population and component summaries. Run `python docs/benchmarks/2026-10-18-simd-text-validation/verify.py` without starting any server.

To repeat a short ordinary screen, use `scripts/inspection.py` with the frozen server, frozen `artifacts/candidates/value-kind/inspection`, `--value-kind utf8 --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 --churn-workers 0 --workers 4 --client-workers 8 --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 --server-cpus 0-3 --client-cpus 4-7,20-23 --pin-workers --pin-client-workers` and a fresh output path. Exact commands for every trial are retained in its receipt. Keep candidates sequential and do not profile ordinary timing.

Manual native/bridge services and their convenience binaries were not replaced. The three pre-existing dirty source files were preserved byte for byte.
