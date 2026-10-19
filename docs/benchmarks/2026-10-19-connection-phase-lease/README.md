# Connection phase-local lease: smaller heap, unselected latency tradeoff

Retain the selected 520-byte Connection layout. Moving the HTTP-upgrade read-admission lease into Stage::Upgrade reduces the candidate connection record to 504 bytes on this measured Linux x86-64 build, without enlarging the 320-byte Stage or adding allocations. The ownership representation is plausible and the requested live heap is smaller, but ordinary 100,000-user comparisons do not justify selecting it for this latency-first relay. No runtime source, service or convenience binary changes are adopted.

The candidate removes Connection::handshake_lease and stores that same Option<ReadLease> beside Option<Upgrade>. The upgrade continuation retains it across Interrupted results; successful upgrade takes it into the existing decoder read budget. Variant field order preserves dropping upgrade storage before its reservation. Two internal test fixtures adapt their direct stage construction. No new API, quota, lock, buffer copy, allocator policy or unsafe operation is added. Exactly three src files differ in the clean candidate manifest. Their prototype is archived, not applied to the working source.

Release-mode layout probes measure Connection/Stage/Option<Upgrade>/Option<ReadLease> as 520/320/288/16 bytes for control and 504/320/288/16 for candidate. A separate hypothetical phase enum also measures 320 bytes. These are compiler/target observations, not stable ABI promises. The two probes are in disposable snapshots with cfg(test)-only diagnostics; the ordinary candidate executable has no diagnostic counter or layout output.

## Requested-heap diagnostic

Two sequential replacement-allocator runs populate 100,000 private users plus 8,192 fanout readers, a publisher and a census session. Eight native workers allocate 131,072 connection slots in both runs. Quiescent tcmalloc dumps are captured at empty baseline and population ready. Each fixture also passes its two-second workload, 32/32 offered windows, exact counters/FIFO/census, full retirement and normal child exits. All diagnostic timing/RSS is excluded from ordinary comparisons.

| Requested live heap | Control | Candidate |
| --- | ---: | ---: |
| Empty baseline bytes | 204,305 | 204,305 |
| Population ready bytes | 116,427,110 | 114,326,630 |
| Population ready allocations | 343,717 | 343,717 |

Candidate requested live heap is 2,100,480 bytes lower, about 2.003 MiB. Slot-count arithmetic predicts 16 x 131,072 = 2,097,152 bytes of record reduction (2 MiB). The extra 3,328-byte difference is in generic vector storage and is not attributed specifically to Connection. Raw heap rows sum exactly to their headers. Major socket, exchange_malloc, hash-table and shared-byte groups are unchanged; generic backing-vector growth decreases by the total heap difference. This demonstrates a requested-heap saving under the diagnostic allocator, not an ordinary RSS reduction or peak-memory bound.

## Ordinary response-time comparison

Four sequential ordinary runs use control/candidate/candidate/control order. Each holds the same 100,000 private users and 8,192 global observers, with eight churn workers paced at 16 complete lifecycles/s each. Native and both load clients have eight individually pinned workers on disjoint core sets. Measurement lasts two seconds, plus population setup, proof scans, CSV generation and exhaustive teardown. Private clients contribute eight observations each (800,000/run); observers contribute 128 each (1,048,576/run) from 32/32 offered four-message windows. All 256 offered churn lifecycles complete in each run. Inspectors and extra private pairs are disabled; loopback/no TLS, normal allocator, heartbeat/rate limits disabled. Builds/tests/profiles/analysis do not overlap ordinary runs.

| Run | Ready RSS KiB | Native CPU s | Median user observed max ms | p99 across user maxima ms | Worst private ms | Worst fanout ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Control 0 | 114,092 | 6.21 | 2.136521 | 14.131358 | 18.156847 | 33.139432 |
| Candidate 1 | 114,480 | 6.57 | 4.171555 | 16.425144 | 20.483191 | 35.335068 |
| Candidate 2 | 116,564 | 6.54 | 4.664140 | 15.301834 | 17.838463 | 35.103892 |
| Control 3 | 114,304 | 6.49 | 3.317427 | 16.461230 | 19.261932 | 34.739980 |

Both candidate median user maxima and CPU values exceed both controls. Candidate worst fanout also exceeds both controls; private population p99 and worst values overlap. Neither candidate RSS is below either control. Smaller requested records do not establish lower resident memory. These finite runs do not prove the layout causes a universal regression, but give insufficient reason to trade response behavior for roughly 2 MiB of requested heap. Keep the existing representation rather than adding padding/reordering variants to chase the result.

All ordinary runs pass full payload/sender/FIFO/census/counter checks, zero routing-copy and error/pressure/rejection counters, final zero connection/session/queued/variable/read-reservation/output-capacity/slot gauges and normal exits [0,0,0]. The candidate passes all 114 clean library tests in 0.52 seconds, strict library/test Clippy and all 61 JavaScript/native/legacy/idle tests in 1.123 seconds, with six checked gauges zero. Correctness is preserved in these checks; performance selection remains negative.

There are 12 configured traffic seconds across six fixtures. Setting up and exhaustively retiring 108,194 connections takes additional wall time, so this is not a twelve-second complete test. No soak, TLS/WAN claim, long-term per-user p99 guarantee or new default setting follows. With eight samples, empirical per-user p99 equals the observed maximum. Population statistics weight users, not pooled packets; timings include client/kernel/relay work. CPU spans the runner's measurement gates, RSS is a ready-gate snapshot, and profile requested bytes exclude allocator metadata, socket kernel memory and stacks.

## Evidence

Frozen control/candidate manifests, exact commands/receipts, full heap captures/analysis, source patch and test logs are archived here. The native control is worker-count-20261019-current; its native sources remain current while intervening bridge-only changes are unused by this executable. The layout probes add only temporary test diagnostics. The current worktree's pre-existing Rust observer-lock and C++ edits are excluded and preserved byte-for-byte.

Run `python verify.py --check-local-csv` from any directory on this checkout to verify source hashes, diagnostics, raw heap totals, all ordinary gates and recomputed user/fanout CSV summaries. Without that option, the verifier checks the portable archived receipts/heap/source evidence only. Raw CSVs and window JSON stay at their recorded artifacts prefixes, SHA-pinned by receipts; they are deliberately not duplicated in this report. The ordinary four commands are retained in runs.log. The archived heap driver/analyzer resolve scripts/artifacts relative to their original repository location; copy them into artifacts and select fresh receipt/heap prefixes to rerun, keeping diagnostic and ordinary runs separate.
