# Measured performance and resource decisions

## Scope

Measurements on 2026-10-15 used a Linux x86-64 host with an AMD Ryzen AI MAX+ 395,
Rust 1.91.1, release builds, four server workers, four load-client workers, and
loopback TCP without TLS. Rate limiting and heartbeats were disabled only for the
benchmark. Normal service defaults retain those protections.

Each candidate used a fresh server process. Candidate order reversed on alternate
repetitions. Candidates ran sequentially, without concurrent compilation or load
tests. The load generator prepared encoded frames outside the timed interval and
checked every delivered byte and its sequence. Latency includes client send,
network, relay, and client receive/validation time. This is not an isolated routing
kernel benchmark or a production capacity guarantee.

## Read-buffer decision

The demonstrated bottleneck was idle connection memory: eager read storage grew
with every connection. We compared 128 KiB (the library default), 16 KiB, and 4 KiB
using identical workloads. The selected default is **4 KiB**, configurable through
`--read-buffer-bytes` / `WAR_READ_BUFFER_BYTES`.

Median of three fresh-process measurements per candidate:

| Workload | 128 KiB read buffer | 16 KiB read buffer | 4 KiB read buffer |
| --- | ---: | ---: | ---: |
| 2,000 idle connections: server RSS | 270.92 MiB | 48.78 MiB | 25.25 MiB |
| 128 clients / private / 256-byte payloads | 864,244 deliveries/s | 1,065,267 deliveries/s | 1,194,581 deliveries/s |
| Private workload p99 latency | 4.03 ms | 3.73 ms | 3.83 ms |
| 32 clients / private / 64 KiB payloads | 107,572 deliveries/s | 126,875 deliveries/s | 123,945 deliveries/s |
| Large-payload workload p99 | 2.91 ms | 2.48 ms | 2.08 ms |
| 64 clients / broadcast / 256-byte payloads | 1,732,624 deliveries/s | 1,769,719 deliveries/s | 1,661,716 deliveries/s |
| Broadcast workload p99 | 0.96 ms | 0.81 ms | 0.90 ms |

Idle RSS fell **90.7%**, about **245.7 MiB** at 2,000 sockets. It excludes the load
generator and kernel socket memory. The connection count was verified against the
server metric before sampling memory. The result is consistent with avoiding
roughly 124 KiB of initial read capacity per socket plus allocator effects.

Throughput is noisier: for example, 4 KiB private/small-payload runs ranged from
882,874 to 1,323,579 deliveries/s, while the 128 KiB runs ranged from 827,950 to
1,054,653. Broadcast ranges overlap substantially. These short runs do **not**
establish a stable throughput win or loss; the firm reason for the default is
the repeatable memory reduction. Larger read buffers remain an explicit tuning
option for workloads with different frame sizes and concurrency.

Private runs used 64 pairs × 1,000 messages (256-byte payloads), or 16 pairs × 200
messages (64 KiB). Broadcast runs used one sender and 63 recipients × 500 messages.
The outstanding-message window was 16. Each successful run verified exactly
64,000, 3,200, or 31,500 deliveries respectively.

Raw records, binary hashes, host details, CPU times, and per-run metrics are in
[benchmarks/2026-10-15-nodelay-buffers.json](benchmarks/2026-10-15-nodelay-buffers.json).
The final default selection and the separate-output-budget fix came after this
comparison; both buffer candidates in the comparison used the same measured
binary and explicit configuration. The table compares configurations, not C++
against Rust or JSON against CBOR.

## Transport diagnostic

An initial measurement had approximately 41 ms p99 latency in small private
messages. The load client's default TCP settings enabled Nagle, which interacted
with the small-message pipeline. The server already set TCP_NODELAY. The harness
now sets it explicitly too; `--nagle` retains the diagnostic control.

A separate comparison used identical binaries, 4 KiB server buffers, fresh
servers, and alternating client Nagle/TCP_NODELAY order. Raw results are in
[benchmarks/2026-10-15-tcp-diagnostic.json](benchmarks/2026-10-15-tcp-diagnostic.json).
Median p99 fell from 41.30 ms to 5.22 ms in that diagnostic. This is a load-client
transport setting comparison; it is not attributed to a server routing change.
The initial Nagle-enabled records remain in
[benchmarks/2026-10-15-read-buffer.json](benchmarks/2026-10-15-read-buffer.json) for
the diagnostic trail. They are superseded for interpreting relay throughput.

## Memory and concurrency contracts

One immutable encoded frame is shared across a broadcast's recipient queues.
Validation does not deserialize application objects. Per-peer byte reservations
survive dequeue until the socket write finishes and release automatically on
failure, cancellation, or queue destruction. A process-wide reservation budget
also limits aggregate queue payloads; variable storage has its own aggregate cap.
Those caps are logical payload limits, not a promise about complete RSS. See
[operations.md](operations.md) for incomplete incoming frames and kernel buffers.

Routing uses one registry read lock and nonblocking queue attempts. Membership,
variable writes, and ID reassignment take its write lock. This is intentionally
simple; no sharding, alternate hash table, batching, or generated dispatch was
introduced without an observed bottleneck. Full shutdown suppresses peer
disconnect broadcasts, avoiding quadratic traffic among closing sessions.

## Reproduce

```sh
cargo build --release --bins --example load
python3 scripts/benchmark.py --buffers 131072 16384 4096 \
  --output artifacts/buffers.json
python3 scripts/benchmark.py --buffers 4096 --workloads private-256 --nagle \
  --output artifacts/nagle-control.json
```

Run other diagnostics separately from timing. The current evidence does not cover
TLS overhead, slow/lossy networks, sustained multi-hour churn, hundreds of
thousands of sockets, distributed routing, or browser rendering. It does not
confirm the upstream README's million-connection claim. Next measurements should
extend workload duration, active-connection count, and adverse-client behavior
before adding structural complexity.

The later [stability investigation](stability.md) adds a 30-second traffic run,
20-second reconnect churn, and a 12-second unread-recipient case. It records two
close-lifecycle failures and their fixes, including the measured throughput
tradeoff and the distinction between concurrent sockets and cumulative reconnects.

## Reusing cancellation waits

The close/eviction fixes initially recreated cancellation and watch futures for
every read-loop iteration and every output. A separate user-cycle profile of
`c06c37b` found `Notified` polling/dropping, waker cloning/dropping, and repeated
watch registration among the largest samples. That diagnostic is in
[close-output-profile.json](benchmarks/2026-10-15-close-output-profile.json).
It sampled user-space cycles, lost 21 samples and four chunks, and was run
separately from the comparisons; its shares are not whole-system CPU fractions.

One shutdown future and one terminal close-reason future now remain pinned for
each session. The read loop and output operations borrow those same futures.
Their completion still exits the session, and a cancelled partial write still
discards its transport. No queue, validation, timeout, or Close contract changed.
This follows Tokio's documented
[approach to resuming a pinned future](https://tokio.rs/tokio/tutorial/select).

Three rotating repetitions per binary used the same frozen verifying client,
64 clients (32 private pairs), 1,024-byte payloads, window 16, 15 seconds, four
workers per process, heartbeat enabled, and inbound rates disabled. Server
CPUs 0–3 and client CPUs 4–7 were distinct physical cores on this host. No
profiling or compilation overlapped the final comparison.

| Binary | Median deliveries/s | Range | Server CPU ns/delivery | p99 upper bucket |
| --- | ---: | ---: | ---: | ---: |
| Initial port, `1b975de` | 1,495,207 | 1,470,864–1,536,614 | 1,932 | 1.049 ms |
| Correct Close/cancellation, `c06c37b` | 1,384,284 | 1,344,703–1,398,560 | 2,135 | 1.049 ms |
| Same fixes, reusable waits | 1,623,686 | 1,586,659–1,634,250 | 1,771 | 1.049 ms |

That is **17.3% more verified deliveries/s and 17.1% less server CPU per delivery**
than the fixed-close control in this workload. CPU time includes connection
setup/teardown; throughput excludes setup. The initial release is only a
diagnostic control because it lacks the lifecycle fixes. An earlier independent
10-second comparison also reproduced the cancellation regression. Raw records,
hashes, core topology, and per-case traces are in
[reused-waits-comparison.json](benchmarks/2026-10-15-reused-waits-comparison.json)
and [close-cost-comparison.json](benchmarks/2026-10-15-close-cost-comparison.json).

Follow-up checks verified 846,758 connection lifecycles in eight seconds and
245,807 healthy deliveries alongside an unread recipient in five seconds, with
correct disconnect notification and complete cleanup. See
[reused-waits-lifecycle.json](benchmarks/2026-10-15-reused-waits-lifecycle.json).
Three 2,000-idle-socket samples per binary measured median RSS of 25.54 MiB for
the control and 26.05 MiB for reusable waits: about 0.50 MiB extra for persistent
session state. See [reused-waits-idle.json](benchmarks/2026-10-15-reused-waits-idle.json).
These remain local measurements, not a hosted capacity claim.

To compare frozen binaries with the same client:

```sh
python3 scripts/compare.py --candidate before=/path/to/before/war-relay \
  --candidate after=/path/to/after/war-relay --stress /path/to/frozen/stress \
  --repetitions 3 --seconds 15 --output artifacts/comparison.json
```
