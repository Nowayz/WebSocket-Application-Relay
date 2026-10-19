# Borrow names for channel-page sort keys — 2026-10-19

Selected: encode a channel-page sort key directly from the borrowed registry name, then construct the owned SnapshotName only after the cursor filter passes. Previously every scanned name was copied into an owned inline name before computing the key, including rows immediately discarded by the cursor. The row still owns its name and scalar metadata before the read guard is released. Standard-base64 ordering, exclusive cursors, one coherent view, the 1,000-row maximum and bounded two-page selection vector are unchanged. No new allocation, queue, index, cache, persistent storage, lock or public setting is introduced.

The earlier excluded-key cutoff/final-partial-partition refinement was found in the original batched-page report and not repeated. It regressed that component screen. This candidate retains unconditional bounded partitions; it changes only the sort-key input and when owned name metadata is constructed. The current decision table now links the prior rejected refinement explicitly.

## Component measurement

The previously used in-module ignored libtest creates 8,192 subscriptions plus the global channel. It independently checks a 1,000-row first page, 16-row tail and empty cursor against the complete listing, warms each case with ten queries, then times nine batches of 100 queries. Fresh processes run control/candidate/candidate/control, pinned to CPU 7. Hash seeds vary naturally. All four processes pass exact result and retirement assertions. Builds finish before timing; no profiler or other benchmark overlaps ordinary measurements.

| Median batch time per query | Controls, microseconds | Candidates, microseconds |
| --- | ---: | ---: |
| First 1,000 rows | 288.856, 282.217 | 273.200, 275.194 |
| 16-row tail | 160.550, 160.907 | 136.375, 136.996 |
| Empty cursor result | 159.851, 160.001 | 135.383, 136.667 |

Comparing the means of the two process medians gives approximately 4% lower first-page elapsed time and 15% lower tail/empty elapsed time. These are whole channel_page component timings, including final ordering/output conversion, not direct read-guard spans or end-to-end HTTP latency. Scanning remains proportional to registry size. The native runtime uses the same routine without the ignored measurement helper; added comments/formatting do not change its algorithm.

## Concurrent acceptance

Four uninstrumented processes run in the same reversed order, each with two seconds of traffic: 8,192 listing rooms, 64 private pairs, two HTTP inspectors requesting 1,000-row first pages every 10 ms, and four churn workers. Four native workers use CPUs 0–3; eight client workers use 4–7,20–23, individually pinned. Full traversal is checked before/after; private sender/body/FIFO, membership, fields, counters and final retirement are verified. All four complete 400 timed HTTP queries. Setup and cleanup add wall time.

| Run | Native CPU-seconds | Private deliveries/s | Private maximum ms | HTTP maximum ms | Churn cycles |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 5.06 | 2,058,807 | 11.374 | 9.325 | 4,362 |
| Candidate 1 | 5.25 | 2,194,603 | 3.475 | 5.216 | 4,700 |
| Candidate 2 | 5.20 | 2,158,098 | 3.133 | 4.313 | 4,625 |
| Control 2 | 5.23 | 2,213,370 | 3.795 | 4.225 | 4,749 |

The returning control overlaps the candidates and improves on the first control. These finite mixed results establish compatibility under concurrent load without proving a whole-relay CPU, throughput or latency win. The workload has unpaced private/churn work, so different useful-work totals preclude treating absolute CPU as fixed-work efficiency. There is no 100,000-user SLA or C++ comparison claim here. The selected benefit is the isolated management-page component improvement with no added allocation traffic or algorithmic complexity.

## Allocation and source checks

A separate tcmalloc diagnostic captures only the query phase of the same component helper, excluding setup/destruction. Both variants record exactly 1,831 allocation events and 262,080,681 cumulative requested bytes, with zero live object/byte delta. All raw stack rows sum to their heap headers. Empty results remain allocation-free under the existing behavior; populated pages keep the prior bounded vector tradeoff. Diagnostic timings are excluded. An initial heap driver hit its ten-second gate watchdog because TextIO prefetched a marker beyond select readiness; the owned child was killed/reaped. Its source remains retained. The corrected driver uses os.read and fresh v2 paths; only its complete captures are accepted.

The selected source passes all 133 library tests in 10.04 seconds, including independent page traversal over limits 1/2/7/1,000, raw binary/Unicode/empty/prefix names, and metadata ownership after registry destruction. The existing bridge welcome-timeout test accounts for ten seconds. Clippy lib/tests with warnings denied passes. JS interoperability passes all 65 checks in 1.125 seconds of test execution and all six checked resource gauges retire. Build/install/cleanup add wall time.

Native control/candidate inputs differ only in src/relay/listing.rs. Both have fresh:false release build receipts and frozen hashes. Separate frozen libtest artifacts have the exact same ignored helper appended; their manifests differ only in that file. The three existing dirty runtime files are excluded and preserved. No manual service or convenience binary is replaced. The selected measured native executable retains its original comparison path: artifacts/candidates/page-borrowed-key-20261019-native-candidate/war-relay.

Run `python docs/benchmarks/2026-10-19-borrowed-page-key/verify.py` to audit saved source/build/profile boundaries and recompute component medians and mixed gates. Scripts, complete receipts, source variants and compressed profiles are retained; repetition needs fresh output paths and serial builds/runs. No multi-hour campaign was needed.
