# Measured performance

These are the current WebTransport results. They come from one development host
(loopback, desktop session), short runs and paired comparisons; identical runs
vary by several percent. Treat them as a description of this relay on that host,
not a capacity promise. [The short-test guide](performance-guide.md) has the
recipes, and [operations](operations.md) explains the budgets and the 100,000-session
profile.

## Throughput and CPU

Closed-loop workload: four relay workers on CPUs 0-3, the inspection client on
CPUs 4-11 with eight workers, 8,192 idle rooms, 16 messages in flight per
sender, 1 KiB values, 4-second windows.

| Traffic | Deliveries per second | Relay CPU per delivery |
| --- | ---: | ---: |
| Private (64 pairs) | about 1.48 million | 1.90 µs |
| Room fanout (one sender, 63 readers) | about 1.55 million | 1.18 µs |

The relay is not CPU-bound here: it used 2.4 to 3.2 of its 4 cores on private
traffic and 1.8 on fanout, with no thread near a full core. Delivery is set by
the round trip, which is why pinned workers (shorter wake-ups) raised private
delivery 14 to 16% over unpinned ones at equal CPU per delivery.

Under saturating load with 32 pairs and 1,024 fanout readers, relay CPU per
delivery was 2.0 µs for private routing, 1.6 µs for room fanout and 2.2 µs for
mixed traffic. Paced load of 400,000 deliveries cost 5.6 s of relay CPU, and a
low-rate probe measured 0.22 ms median and 0.40 ms p99 round-trip latency.

A profile of one private delivery (about 2.0 µs) splits it roughly as: QUIC
packet, acknowledgement and stream work in user space 0.53 µs; kernel 0.42 µs
(UDP sends several packets per system call); encryption 0.36 µs (AES-GCM and
header protection); timers, scheduling and allocator 0.17 µs; shared-buffer
reference counting about 0.1 µs; memory copies 0.13 µs; relay routing 0.13 µs.

## Memory

With 100,000 idle sessions joined in rooms of 16, the relay process held about
1,670 MiB, and process plus kernel socket memory about 1,668 to 1,674 MiB: all
sessions share one UDP socket. After the population retires, jemalloc returns
pages over a few seconds and resident memory falls to about 1,350 MiB.

## Impaired networks

A 1 MiB reliable transfer through a network namespace with netem and a 1500-byte
MTU:

| Profile | Transfer time |
| --- | ---: |
| 2% loss, 20 ms each way | 0.96 to 1.14 s |
| 3 ms jitter, 50 ms each way, no loss | 1.36 to 1.47 s in 24 runs |
| Heavy reordering, 20 ms each way, 2% loss | 4.1 to 5.3 s |

Datagram state updates sent alongside the transfer stayed fresh: p99 age of the
newest state was 69 to 180 ms across the three profiles. Time-threshold-only
loss detection, held early 1-RTT packets and deferred connection-ID retirement
(see [vendor/README.md](../vendor/README.md)) produced the jitter result; before
them the same transfer took 5.8 to 8.3 s.

## Rejected experiments

These were measured and not kept: the mimalloc allocator, shorter Tokio
global-queue intervals (31, 16, 8 and 4), smaller fanout quanta (256 and 64),
faster acknowledgement policies, a 5/4 loss time threshold and a direct protocol
owner without per-connection tasks. Each lost throughput, CPU or memory in
paired runs, or added complexity without a complete-workload gain.

Dated measurement records from the WebTransport work are in
[reports/](reports/README.md).
