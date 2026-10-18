# Empty management pages: defer unused allocation

Selected source change: `src/relay/listing.rs` now reserves its two bounded
selection buffers on the first qualifying channel. A cursor beyond the final
channel returns an empty vector without allocating those buffers. Populated
pages retain one exact reservation per buffer, the same page bound, coherent
registry read, base64 ordering, and owned snapshot rows. Routing bodies, FIFO,
fanout scheduling, and resource limits are unchanged.

Control source is clean revision `912ed0e`; candidate differs only in that
reservation placement (plus the separately archived ephemeral libtest screen).
The three pre-existing workspace modifications were excluded from builds and
verified unchanged. Frozen executables remain under
`artifacts/candidates/page-lazy-{control,direct}/`; executable hashes are in the
HTTP records. This change is adopted in source; the long-running selected
native and bridge processes were not restarted.

## Allocation finding

A release libtest creates 8,192 subscriptions, verifies a first 1,000-row page,
a 16-row tail and an empty page against the independent complete listing, then
runs 910 calls for each case. A separate tcmalloc diagnostic brackets only that
query phase with signal-triggered heap dumps; timings from this diagnostic are
excluded. Cumulative allocated traffic is not retained RSS.

| Query phase | Control | Candidate | Reduction |
| --- | ---: | ---: | ---: |
| Allocations | 7,291 | 5,471 | 1,820 |
| Allocated bytes | 262,779,561 | 189,979,561 | 72,800,000 |

This matches two allocations and 80,000 bytes removed for each of 910 empty
queries at page limit 1,000. Normal pages still allocate bounded page storage.
No registry-sized cache, new index, or persistent memory was introduced.

Four ordinary component runs in control/candidate/candidate/control order took
less than a second each. Median batches were approximately 415 us versus
390–392 us for first pages and 193 us versus 165–168 us for empty pages.
Fresh hash-map seeds affect traversal and replacement work, so these are
observations, not a portable speedup claim. All queries still scan the registry:
this change does not solve the O(channel count) traversal or read-lock duration.

## Concurrent HTTP acceptance

Four separate runs, control/candidate/candidate/control, each used a three-second
measured phase: 8,192 listing rooms, 64 private pairs, two inspectors at 10 ms,
1,000-row HTTP first pages, four server workers and eight client workers.
Server CPUs 0–3; client CPUs 4–7,20–23; workers individually pinned. No profiling
or builds overlapped these retained `http-quiet` measurements. Setup, full
pagination verification and cleanup are additional to the three-second phase.
All four passed exact response, ordering, traffic, counter and cleanup checks.

| Observation | Control runs | Candidate runs |
| --- | --- | --- |
| Verified private messages/s | 2.198–2.266 million | 2.271–2.282 million |
| Server CPU seconds / three-second phase | 8.17–8.21 | 8.22–8.30 |
| Private histogram p99 upper bound | 2.097 ms, 2.097 ms | 2.097 ms, 1.049 ms |
| HTTP queries verified per run | 600 | 600 |

The full-page path remains comparable; these short runs do not establish a
whole-relay speedup or a tail-latency SLA. They are an acceptance check for the
common path, not a replacement for the existing 100,000-user fairness evidence.
The guaranteed benefit here is elimination of unused empty-page allocations.

An initial HTTP launch failed because the frozen executable lacked execute
permission; its record is retained. After correcting that fixture, an initial
four-run batch passed but overlapped a test compilation and is excluded from
performance results. The retained `http-quiet` batch ran afterwards without
that interference.

## Reproduction and verification

The archived `page-lazy-benchmark-module.rs` is a disposable in-module libtest,
not production code. Append it to `src/relay/listing.rs` in independent clean
control/candidate snapshots, build `cargo test --release --lib --no-run`, and
freeze each compiler-reported libtest executable. Run sequentially:

```sh
taskset -c 7 PATH_TO_SCREEN relay::listing::allocation_screen::management_page_screen --exact --ignored --nocapture
```

`page-lazy-heap.py` uses those frozen artifacts and the existing
`scripts/inspection.py::dump_heap` helper with `/usr/lib/libtcmalloc.so`.
Heap before/after records and compressed selected dumps are archived. HTTP
records include exact commands, source/executable hashes and affinity checks;
`summary.json` extracts the relevant observations. The helper paths are local
reproduction fixtures, not a new supported public benchmark CLI.

Existing pagination tests cover independent full-listing equivalence, cursors,
limits and owned metadata after mutation. Validation logs and `SHA256SUMS`
accompany the raw evidence. No multi-hour tests were used.

Validation: clean candidate library 80/80 tests passed; workspace library 83/83 passed (including pre-existing tests). Clean candidate library/native Clippy passed with `-D warnings`; Rust formatting and diff whitespace checks passed.
