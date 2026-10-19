# Performance findings retained from the comparison phase

These are historical measurements, not freshly rerun claims about the current
checkout. Full original reports, raw receipts, source patches and build digests
are available at Git commit `587c1cd`. The maintained tree omits the temporary
C++ implementations and the large benchmark archive. Use, for example:

```sh
git show 587c1cd:docs/benchmarks/2026-10-17-threeway-current/README.md
git show 587c1cd:docs/benchmarks/2026-10-17-rust-owner-fanout/README.md
git show 587c1cd:docs/benchmarks/2026-10-19-global-shutdown-indexes/README.md
```

## What the C++ comparisons taught us

The six-repeat three-way study on 2026-10-17 used four server cores, identical
checked 1 KiB values, 16-message windows and 8,192 idle rooms. Median private
forwarding was 2,205,873 deliveries/s for original C++, 2,065,901 for the then
selected Rust, and 1,895,091 for modern C++. Original C++ led private CPU efficiency
in that workload. With 63 fanout readers, modern C++ led at 1,882,604 deliveries/s,
versus 835,885 original C++ and 670,403 then-selected Rust. One Rust query pacing
failure remained in the raw timing and was not counted as full acceptance.

A subsequent matched owner-worker fanout campaign selected Rust commit `7290017`.
It delivered 2,494,487 fanout messages/s versus 644,363 previous Rust, 1,576,514
modern C++ and 687,213 original C++. Corresponding CPU microseconds/delivery were
0.604, 0.748, 0.937 and 0.684. Private throughput was essentially unchanged versus
previous Rust in that campaign. Different campaigns used different trials and
placements; the later rows do not overturn all earlier private results or prove
a language-wide ranking.

The decisive improvement was parallelizing multicast output on recipient owners
with shared payloads and bounded vectored batches. More server threads alone
had left one publisher doing nearly all output work.

## Later Rust results

| Change / source report at `587c1cd` | Measured result and limit |
| --- | --- |
| `2026-10-18-mixed-100k-fairness` | Median user's p99 fell from 11.99 to 2.03 ms in the measured competing workload. Fanout-window tails and quiet-path CPU tradeoffs were recorded separately. |
| `2026-10-19-departure-fifo` | Bounded departure work reduced worst witness RTT from 107.128 to 4.571 ms at 100,000 recipients. Ordinary mixed-load CPU was effectively flat; queue-pressure fallback stayed synchronous. |
| `2026-10-19-retirement-wake-coalescing` | A release component census reduced overlapping publication wakes from 1,001 to 1. Component publication time fell about 26%; no general relay-throughput gain was claimed. |
| `2026-10-19-observer-revocation` | Fixed old notices arriving after unsubscribe acknowledgments. Initial correct barrier cost about 373 ms while draining 100,000 ordinary-room recipients. |
| `2026-10-19-observer-scoped-barrier` | Restricting that barrier to global-channel jobs reduced acknowledgment to 1.14–1.19 ms in the ordinary-room screen, versus 377–378 ms control. An unrelated overlapping witness fell from about 376 ms to 0.73–1.65 ms. |
| `2026-10-19-global-shutdown-indexes` | Releasing global listener indexes before bulk retirement reduced median end-to-end cleanup from 3.834 to 2.650 s (30.9%). Four runs each verified 100,000 normal Close frames with the unchanged ten-second default grace. |

The short-grace shutdown negative control verified only 72,846 normal Closes
with a two-second setting, despite exit status 0. Successful process exit is not
a substitute for checking the client matrix. The global-observer revocation
screen still showed roughly 338–350 ms stalls; that live-control problem remains.

## Rejected approaches and measurement lessons

Per-worker departure queues reordered notifications and were rejected. A
constant-metadata BTree retirement cursor survived tight queue pressure but did
not justify its normal-load cost. Buffered multicast reduced some CPU/throughput
costs without establishing consistent private-user tails. Allocator preloads,
token lookup variations, alternate writers and timer fast paths were retained
only as experiments unless complete-workload evidence supported selection.

CPU accounting must include the kernel boundary explicitly. Throughput must count
verified deliveries. Compare the same protocol work, worker placement, payloads,
read windows, inspection rate and cleanup contracts. A single fast run, a component
speedup, or a nominal 100,000-session census cannot establish per-user fairness.
Future decisions use [short native recipes](performance-guide.md) and prior Rust
revisions, with the original comparison phase retained in Git history.
