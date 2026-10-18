# Bounded batched channel-page selection

Source change `3ee013c` replaces the per-row binary heap in `channel_page` with
a single bounded vector. When two pages of candidates accumulate, partition
at the page limit and discard the upper half. Discarded rows already have at
least one page of smaller keys and cannot enter the final result. Sort the
remaining candidates after releasing the registry guard and return the first
page. This removes a separate snapshot vector and its slot bookkeeping.

No index, cache, background task, new limit, or persistent memory was added.
Pages still use one coherent registry view, standard-base64 key ordering,
exclusive cursors, owned metadata and the existing 1,000-row maximum. Routing
payloads, FIFO, fanout scheduling and zero-copy delivery are unchanged. Empty
pages retain the previous no-allocation behavior. Temporary metadata is
bounded to twice the requested page size, independent of channel count.

This is adopted in source; long-running selected native/bridge processes were
not restarted. Their older executable identities remain in the performance
guide. The three pre-existing dirty source files were excluded from clean
builds and verified byte-identical afterwards.

## Establish the bottleneck

Disposable clean source snapshots came from `e8e5e27` (runtime unchanged from
`79195d7`). A diagnostic build times immediately before acquiring the registry
read lock, immediately afterwards, and after releasing it. Each first-page
call prints the population and two nanosecond durations **after unlocking**.
No timing probes enter production. Clock/logging overhead affects diagnostics;
sub-microsecond waiting should not be interpreted as a precise lock cost.

Two-second runs used 8,192 listing rooms, 64 private pairs, two inspectors
polling 1,000-row first pages every 10 ms and either zero or four churn workers.
Four server workers used CPUs 0–3; eight client workers were pinned individually
to 4–7,20–23. Each case also verifies full pagination before/after the workload,
traffic content, FIFO, counters and complete retirement. There are 402 logged
first pages per run: 400 workload queries and two quiescent checks. Reported
lock summaries include all 402; they are diagnostic, not an application SLA.

| Diagnostic | Median read acquisition wait | p99 wait | Median guard span | p99 guard span |
| --- | ---: | ---: | ---: | ---: |
| Heap, no churn | <1 us | <1 us | 636 us | 1,256 us |
| Heap, four churn workers | <1 us | 629 us | 575 us | 795 us |
| Batched, four churn workers | <1 us | 356 us | 565 us | 1,120 us |
| Heap return run, four churn workers | <1 us | 514 us | 621 us | 1,139 us |

Scanning/selecting holds the read guard for measurable time. Churn introduces
occasional acquisition waits. The batched diagnostic has a modest lower median
guard span than both heap/churn diagnostics, but not uniformly better guard
span tails. This does not directly measure writer wait or establish causality
for all latency changes.

## Uninstrumented component screen

A separate release libtest creates 8,192 subscriptions plus the global channel,
then independently verifies the first 1,000 rows, a 16-row tail and an empty
cursor against the complete listing. Each case warms up ten queries then times
nine batches of 100 queries. Each fresh process lasts under one second. The
same CPU (7), workload and opaque result checks are used in
control/candidate/candidate/control order. No other builds or benchmarks run
concurrently. Fresh hash seeds can change scan/selection work.

| Process median batch time | Heap control runs | Batched runs |
| --- | --- | --- |
| First 1,000-row page | 383.7, 388.1 us | 288.6, 285.7 us |
| 16-row tail | 167.2, 169.1 us | 160.4, 164.6 us |
| Empty page | 166.1, 168.2 us | 161.9, 163.7 us |

First-page time fell approximately 25% in this screen. Both implementations
still scan the registry; this does not make traversal independent of channel
count. The complete operation includes ordering and output conversion, so the
component gain is not identical to guard-span reduction.

## Concurrent HTTP/private/churn acceptance

Four uninstrumented two-second measured phases use the larger workload above
with four churn workers, in control/candidate/candidate/control order. Setup,
full traversal verification and cleanup are outside the two-second phase.
All runs passed content, ordering, counter, affinity and retirement checks.

| Observation | Heap control runs | Batched runs |
| --- | --- | --- |
| Private messages/s | 2.016, 2.090 million | 2.131, 2.094 million |
| Private p99 histogram upper bound | 4.194, 4.194 ms | 2.097, 2.097 ms |
| Churn lifecycles/s | 2,091, 2,227 | 2,270, 2,241 |
| Server CPU seconds | 5.04, 5.14 | 5.23, 5.16 |
| Verified HTTP queries | 400, 400 | 400, 400 |

These finite observations favor the candidate for responsiveness in this
fixture, but do not establish a universal throughput win, CPU reduction or
long-term latency guarantee. Useful throughput and CPU consumption both vary.
They supplement rather than replace the existing 100,000-user fairness report.
No C++ comparison is claimed here.

## Allocation and memory tradeoff

A separate tcmalloc diagnostic captures cumulative allocation counters before
and after only the query phase: 910 queries each of first, tail and empty.
Setup/destruction are excluded. Diagnostic timings are not mixed with ordinary
results. The selected dump records and compressed raw profiles are archived.

| Query-phase allocation traffic | Heap | Batched |
| --- | ---: | ---: |
| Allocation calls | 5,471 | 1,831 |
| Allocated bytes | 189,979,561 | 262,080,681 |

The 3,640-call reduction is two fewer allocations for each of 1,820 populated
queries. Empty queries allocate no page storage in either version. This build
reuses the selected vector's allocation during output collection, so the
populated-page path allocates once. Allocation reuse is compiler/build evidence,
not a public API promise.

The tradeoff is **more allocated bytes**, despite fewer calls. At limit 1,000,
this build allocates a 2,000-entry vector with 72-byte key/snapshot rows:
144,000 bytes per populated query. Heap control allocates 128,000 total bytes
for a full page and 80,768 for the 16-row tail (two scratch vectors plus output).
The measured aggregate byte increase is 72,101,120, about 38% for this deliberately
mixed first/tail fixture. It is cumulative traffic, not an RSS increase.

This bounded memory premium is retained in exchange for simpler bookkeeping,
fewer allocator calls and faster common first-page selection. There is no
registry-sized allocation. Concurrent requests multiply bounded per-request
storage, as before; no claim of lower peak RSS is made. On this build returned
rows retain the reusable allocation until serialization retires them.

## Rejected extra refinement

A second candidate cached the first excluded partition key to skip later rows
and partitioned the final partial batch before sorting. Although plausible, it
regressed the component screen: first pages around 341–342 us and tail/empty
pages around 290–298 us, versus control roughly 165–168 us for tail/empty. All
four concurrent runs passed, but this clear common-case regression is enough
to reject it. Its exact source and raw runs remain archived. No explanation
based solely on source inspection is asserted for that code-generation result.
The selected implementation uses the simpler unconditional bounded batches.

## Verification and reproduction

Clean candidate library: 80 tests passed. Workspace library, including existing
user changes: 83 passed. Existing independent pagination tests exercise page
limits 1, 2, 7 and 1,000; binary/Unicode/empty/prefix names; cursor traversal;
and ownership after registry destruction. Clean library/native Clippy with
`-D warnings`, Rust 2024 formatting and diff whitespace checks pass.

The raw component module is an ephemeral in-module ignored test, not a shipped
benchmark interface. Append it to each isolated `src/relay/listing.rs`, build
`cargo test --release --lib --no-run`, freeze the reported executable and run:

```sh
taskset -c 7 PATH_TO_SCREEN relay::listing::allocation_screen::management_page_screen --exact --ignored --nocapture
```

Raw HTTP JSON contains exact commands, source/executable hashes and affinity
checks. Probe source files show the timing instrumentation. The heap helper
uses `/usr/lib/libtcmalloc.so` and existing `inspection.dump_heap` signal gates.
Frozen ordinary artifacts are under `artifacts/candidates/page-lazy-direct`
and `artifacts/candidates/page-partition`; diagnostic artifacts have separate
names. Runtime source differs from the measured candidate only in comments and
formatting. `summary.json` extracts metrics; `SHA256SUMS` covers the archive.
No multi-hour tests, live restarts or user-source rewrites were needed.
