# Concurrent observer publications

The [bounded-publication implementation](observer-publications.md) removed long
single-publication membership-lock stalls. The next screen overlaps publications
from multiple connections, alternating channel and private messages. It preserves
the same delivery, revocation, FIFO and bounded-storage requirements.

## Reproducible screen

```sh
python3 scripts/observer-message-fairness.py \
  --server /absolute/path/to/frozen/war-relay --workers 2 --audience 2048 \
  --publishers 8 --kind mixed --payload-bytes 8 \
  --output artifacts/observer-concurrent-short.json
```

Each source occupies a singleton room. Every observer receives every publication;
the ordinary private target also receives all private publications. One observer
must receive the complete source set before revocation starts, proving capture
while the final observer was eligible. That final observer must receive the same
set before its unsubscribe acknowledgment. Immediate re-subscription and a second
round detect replay or missing subsequent messages. Cross-source ordering is
unspecified; source IDs and round values are checked exactly, and each round is
fully drained before the next one. This screen does not replace the pipelined
source-FIFO regressions.

Independent control-reply witnesses and a join overlap each event. Mixed/private
screens use control replies because private echoes would create extra fanouts.
Exact wire/counter totals, quiescent queue/decoder reservations, normal source
Close/EOF and every retained observer's shutdown Close `1001` plus EOF must pass.
The eight-byte value contains the round marker and produces a 19-byte CBOR
envelope for a uint64 sender. Small values keep the concurrent burst within the
fixture's socket buffers; they isolate routing/scheduling rather than force
large-value backpressure. The existing single-publisher default remains 1 KiB.

CI runs the small mixed case alongside the single-class one-slot and combined
room/observer tight-byte-budget cases. The in-process
`alternating_view_cache_distinguishes_equal_sized_generations_and_cached_absence`
test separately checks eligibility across different equal-sized view generations,
cached absence, exact values, re-subscription, queue accounting and release
of old view ownership.

## Rejected scan experiment on 2026-10-19

Revocation scans pending publication views while holding the membership write
lock. The current one-entry weak identity cache reuses a lookup across adjacent
jobs sharing a view; alternating channel/private views can repeatedly miss.
An experiment used two stack cache entries and compared canonical peer pointers
when removing listener membership, avoiding scattered peer-key reads. It added
no persistent peer field, payload copy or heap allocation to those lookups.

A manual release diagnostic paused 64 publications across 100,000 observers.
It measured the actual capture, revoke, cancel and reenable operations with a
counting output sink, excluding socket I/O. Across 12 rounds:

| Median diagnostic time | Control | Pointer lookup only | Pointer lookup and two view entries |
| --- | --- | --- | --- |
| Revocation | 1.621 ms | 1.335 ms | 0.085 ms |
| Capture, revoke, cancel and reenable | 3.236 ms | 2.989 ms | 1.677 ms |

Run this diagnostic explicitly, apart from ordinary tests:

```sh
cargo test --locked --release --lib observer_barrier_event_screen \
  -- --ignored --nocapture --test-threads 1
```

`BENCH_OBSERVERS`, `BENCH_PUBLISHERS` and `BENCH_ROUNDS` override its declared
population, publisher count and repetitions. It is a component diagnostic, not a
capacity test or a native relay latency result.

The native follow-up used the AMD Ryzen AI MAX+ 395 host, 100,000 observers,
eight-byte values, mixed publication classes and two rounds per run. Builds and
measurements were serialized. Clients used CPUs 8–15,24–31; eight-worker servers
used CPUs 0–7, and the single-worker cross-check used CPU 0. These are one
control/candidate pair per profile, with two event samples per version:

| Native profile | Control unsubscribe ACK | Candidate unsubscribe ACK | Control independent join | Candidate independent join |
| --- | --- | --- | --- | --- |
| 16 publishers, eight workers | 5.54–6.25 ms | 3.66–6.03 ms | 6.32–7.32 ms | 4.08–10.69 ms |
| 64 publishers, eight workers | 4.16–10.38 ms | 6.02–11.94 ms | 9.15–11.75 ms | 10.42–12.00 ms |
| 64 publishers, one worker | 31.26–37.09 ms | 24.39–34.49 ms | 78.50–80.50 ms | 63.49–73.57 ms |

All six native runs passed every delivery and all 100,000 retained shutdown
receipts. Each 64-publisher run checked 12.8 million audience deliveries. The
eight-worker 64-publisher runs used 7.96/8.13 server CPU-seconds for control/
candidate; the one-worker pair used 2.79/2.75. The fixture contains closed-loop
witnesses and serial audience verification, so these CPU figures are not equal
offered-rate efficiency measurements.

The experiment is **rejected**. The single-worker pair favored the candidate,
but the main eight-worker profile did not demonstrate a complete-workload win.
The small sample does not establish a regression or a speedup. No production
scan, cache or membership-lookup change is retained. The fixtures, deterministic
coverage and diagnostic remain to evaluate subsequent changes. No new mixed
100,000-user claim is made for this experiment.

The next hypothesis is aggregate worker service time. A ready batch can drive
up to 1,024 connections, each of which can advance a 1,024-recipient publication
before polling fresh readiness again. A per-publication visit bound alone does
not bound the sum of concurrent publication work. Profile and measure an
aggregate routing allowance or interleaving policy before selecting one. Preserve
edge-triggered continuation, output order, shutdown/timer service and completed
offered fanout work. Changing/cold recipient views under churn remain a separate
membership-lock measurement target.

The [worker-scheduling follow-up](worker-scheduling.md) implements and measures
separate fresh/buffered service with aggregate routing allowances, while retaining
the original source turn and small-fanout cutoff.

Control executable SHA-256:
`5f27499f27159a3bbaa9f5de5311bb1b6646719e681723503313e69eed4053c9`.
Rejected candidate executable SHA-256:
`c1baf3d03f8b8163573c015278ab2538cec82c53d285b2d7827795315404a6e7`.
Frozen candidate Cargo inputs, build receipt, client/helper sources, diagnostic
logs and native JSON are local under
`/home/phil/dev/war_next/artifacts/concurrent-observers-20261019/`.
The candidate build directory has a provisional `selected` suffix; the final
selection receipt records its rejection. The retained runtime source remains
that of `0b34e27`; this follow-up adds tests, fixtures and documentation.
