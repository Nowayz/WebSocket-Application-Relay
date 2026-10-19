# Bounded HTTP pages during churn and fanout — 2026-10-18

The selected native source passed three short management/routing acceptance fixtures without requiring a cache, another index or a runtime change. Each traffic interval was two seconds, plus setup, full quiescent page traversal and checked retirement. This is correctness/resource evidence, not a before/after performance comparison or a 100,000-user latency claim.

| Stable extra rooms | Page limit | HTTP inspections | Churn lifecycles | Private deliveries | Fanout deliveries |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 128 | 1 | 400 | 6,901 | 1,018,752 | 2,859,008 |
| 128 | 1,000 | 400 | 7,426 | 973,568 | 2,736,128 |
| 2,048 | 1,000 | 400 | 6,228 | 776,800 | 2,572,288 |

Every fixture has eight private pairs, four churn workers, two HTTP inspectors at 10 ms intervals, 2,048 fanout readers and four messages per window. Four native workers and eight client workers are pinned to disjoint CPU sets. Timed inspections verify the bounded first page; complete name/order/member/variable validation and page traversal occur at quiescent gates. The 2,048-room fixture exceeds twice the 1,000-row limit, exercising partition-and-truncate selection as well as sorting. Names include binary and Unicode cases supplied by the existing client.

The server is the frozen selected outlined writer with management batching already included, SHA-256 da1d1289c0ce0e5264de26ad87b756d5034576ef3cf4126b44c0f2530c1ea006. All receipts carry commands, binary identities, source hashes, counters and affinity verification. Existing source overlays were excluded from this binary. All fixtures pass exact input/output/reuse/lifecycle counters, FIFO/payload checks, normal exits and zero final connection/session/read/output/variable reservations and queue capacity.

A page is coherent for one request. Traversing pages during concurrent membership changes is not a historical registry snapshot; names can appear/disappear between requests. These fixtures validate complete traversal at gates after churn stops, rather than claiming a cross-request snapshot while writes continue. Returned metadata owns no peer or payload bodies, and encoding occurs after releasing the registry guard.

Use the full commands in raw receipts to reproduce, with fresh output names and CPU masks appropriate to the host. No profiler or build overlapped traffic. Workload rates vary with scheduling and page size; the counts are acceptance work, not an isolated pagination speed comparison. Run python verify.py for offline archive verification. Manual services and pre-existing dirty source files were preserved.
