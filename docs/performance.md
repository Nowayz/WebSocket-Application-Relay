# Measured performance and resource decisions

## Scope

Measurements on 2026-10-15 used a Linux x86-64 host with an AMD Ryzen AI MAX+ 395,
Rust 1.91.1, release builds, four server workers, four load-client workers, and
loopback TCP without TLS. The initial read-buffer comparison disabled inbound
rates and heartbeats. The later cancellation and zero-copy comparisons disabled
inbound rates and kept heartbeats enabled. Normal service defaults retain both
protections.

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

One immutable application body is shared across a broadcast's recipient queues;
its small replacement routing prefix is stored separately.
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

## Preserving receive storage through forwarding

Upstream's router replaces the target ID with the sender ID in its received
message. The first Rust port preserved the encoded CBOR value but copied it into
a newly encoded envelope. The WebSocket writer then copied that envelope into
its contiguous output buffer for each recipient. The native forwarding path now
preserves the application body through both stages:

1. Validate the complete received CBOR envelope using borrowed slices and a fixed
   parser stack. Consume its `Bytes` ownership when routing.
2. Encode only `[sender, ...]` into ten bytes of inline storage. Keep a shared
   view of the original encoded application value. Different CBOR integer widths
   require no body movement or protocol restriction.
3. Clone shared body ownership into recipient queues. Reservations include the
   emitted routing prefix and remain charged until writing finishes or aborts.
4. Put the WebSocket header and CBOR prefix into one 20-byte stack buffer. Write
   it and the immutable body through two `IoSlice`s, advancing through partial
   writes without assembling a contiguous payload.

The existing WebSocket library still performs the handshake, masking checks,
unmasking, fragmented-message assembly, and control parsing. The native server
wrapper owns writes sequentially, flushes pending control responses before data,
and disables application output as soon as Close, an error, or EOF is observed.
Only a completed server handshake without framing extensions can enter this
path. A cancelled or failed partial write discards that transport, as before.
The optional legacy bridge still does its necessary protocol translation.

The common native route has no newly assembled application payload buffer and
eliminates the two explicit full-value copies in private forwarding (one routing
copy plus one socket staging copy). Fanout also avoids a staging copy per
recipient. This is **user-space forwarding**, not kernel zero-copy: socket
transfers, unmasking, input-buffer growth, and fragmented-message assembly still
do their required work. No claim of zero allocations for the complete connection
or queue implementation is made.

Borrowing changes retention. Once a connection's largest observed message
exceeds its initial read buffer, shared messages smaller than one eighth of that
high-water mark are compacted once before fanout. Ordinary tiny messages share
the normal slab. Unique receive storage needs no compaction. This conservative
policy prevents a succession of tiny queued views from pinning enlarged read
slabs; it may keep copying small values after one large input. The queue budget
continues to count logical output bytes, not physical allocation capacities.

`war_routing_reused_bytes_total` and `war_routing_copied_bytes_total` expose these
two routing choices. Values are counted once before fanout, excluding relay
commands and unknown targets. Pointer-identity checks also prove that the
original body reaches every recipient queue and the vectored write unchanged.
Other checks cover CBOR integer widths, nonminimal target/array encodings,
WebSocket length boundaries, three-byte partial writes, Close/Pong ordering, and
a 70,000-byte fragmented input while the same socket receives unrelated output.

The final binary was compared with the reusable-waits control from `743c9ca`.
Each size used three alternating 10-second runs per binary, 64 clients / 32
private pairs, window 16, four workers per process, the same frozen validating
client, and distinct server/client physical cores (0–3 / 4–7). Rates were disabled
and heartbeats enabled. Neither profiling nor compilation overlapped these runs.
Traffic queues fit a whole window: the runner uses
`max(1 MiB + 10, 32 * (payload_bytes + 20))` equally for both candidates. These
are harness settings, not changes to product defaults. The unread-client test
retains its deliberate tight queue limit.

| Raw application payload | Before: median deliveries/s | Shared receive body: median deliveries/s | Before: server CPU ns/delivery | Shared body: server CPU ns/delivery | p99 upper bucket, both |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 KiB | 1,598,532 | 1,605,601 | 1,740 | 1,776 | 1.049 ms |
| 64 KiB | 199,854 | 209,143 | 16,234 | 11,784 | 4.194 ms |

The 64 KiB workload used **27.4% less server CPU per verified delivery**, with
**4.6% higher median throughput** in this comparison. At 1 KiB, throughput
changed +0.4% with overlapping run ranges, while CPU per delivery rose 2.1%.
That small-message cost remains a tradeoff; the evidence does not establish a
general small-message speed gain. CPU includes setup/teardown and is sampled at
the process level; throughput excludes setup. These results do not measure C++
versus Rust or CBOR versus another serialization format.

All six final candidate traffic runs verified every delivered byte and returned
to zero sessions, connections, queued bytes, and variable bytes (one persistent
global channel). They routed 49,331,279,504 encoded application bytes in the
1 KiB cases and 409,369,086,000 in the 64 KiB cases. The routing-copy counter was
zero throughout; the reuse counter matched the independently verified delivery
count times the encoded-value size in every run. Raw records, hashes, topology,
per-run counters, RSS, and traces are in
[zero-copy-final-small.json](benchmarks/2026-10-15-zero-copy-final-small.json) and
[zero-copy-final-large.json](benchmarks/2026-10-15-zero-copy-final-large.json).

The final lifecycle rerun completed 917,429 cumulative connection lifecycles with
64 churn workers in eight seconds. A separate five-second unread-client case
verified 259,154 healthy deliveries and exactly one disconnect notification;
the unread session was absent by the first post-overflow sample at 0.276 seconds.
All lifecycle gauges cleaned up. See
[zero-copy-final-lifecycle.json](benchmarks/2026-10-15-zero-copy-final-lifecycle.json).
These are cumulative reconnects, not that many simultaneous connections.

Three additional 2,000-idle-socket samples per binary measured median server RSS
of **26.01 MiB before and 26.88 MiB after**, a **0.86 MiB** increase for the
larger per-message metadata and transport/session state. RSS excludes the client
and kernel socket storage. Three finite fanout checks per binary each verified
all 31,500 deliveries (500 broadcasts × 63 recipients, 256-byte raw values,
window 16). The final candidate's routing-copy counter stayed zero. These short
fanout checks establish acceptance, not sustained broadcast throughput. See
[zero-copy-final-idle-fanout.json](benchmarks/2026-10-15-zero-copy-final-idle-fanout.json).

To repeat the two sizes, use the frozen-binary comparison command above with
`--payload-bytes 1024` and `--payload-bytes 65536`. Run other diagnostics and
compilation separately from each comparison. Longer fanout, mixed active/idle
populations, adverse networks, TLS, and multi-host capacity remain unmeasured.
