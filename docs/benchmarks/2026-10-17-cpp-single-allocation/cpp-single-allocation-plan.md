# Queued body allocation study

Control: accepted modern C++ build from 02aea99 (runtime introduced at 8119669).
Candidate: make_shared_for_overwrite<char[]> for shared immutable queued bodies; full size_t length occupies the previous close-fields union. Delivery remains 40 bytes. Borrowed local output, one copy per retained body, per-recipient accounting and all quotas remain intact.

The accepted-build diagnostic profile attributes sampled work to submit (12.85%), write (12.26%), drain (6.58%) and reconciliation (5.25%); allocator/control-block construction appears in submit call paths. This motivates testing allocation removal but does not predict an end-to-end gain.

Required evidence: actual allocation count and ownership diagnostic; unchanged protocol corpus; single-worker, small/large private, fanout and churn guards; fresh sequential rotated 100k-user trials with exact per-user latency, ready/measured CPU gates and complete resource retirement. Confirm population results independently because private throughput is confounded by connection placement and the retained-copy path. A retained-copy proxy is diagnostic only, not an ownership census.

Before adoption, run ASan/UBSan and TSan corpus plus embedded lifecycle, control and slow-reader runtime checks. Adopt only if measurements justify the ownership representation and responsiveness remains supported. Preserve all observations, including reversals; do not use preliminary allocation diagnostics where compiler elision invalidated the measurement. No ordinary timing overlaps a build, sanitizer, profile or another workload.
