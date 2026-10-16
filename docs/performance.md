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

The later [mixed-size fanout investigation](stability.md#mixed-size-fanout-and-simultaneous-peer-departure)
adds 128-client active workloads and a five-millisecond delayed reader beside an
unread peer. It distinguishes cancellation before the first write poll from
cancellation of a possibly partial frame, records the reproduced mass-departure
reset and its fix, and includes a repeated small-message cost check.
## Retention under repeated unknown-target primers

The `retention` example exercises backing-storage lifetime through real sockets.
Eight 1 MiB values first reach an unread recipient with a 64 KiB requested TCP
receive buffer. The sender then alternates a prebuilt 4 MiB message to an unknown
ID, a tiny value to that recipient, and a verified self-message barrier. The
barrier proves that preceding input was processed while output remains blocked.
After a two-second hold, the client reads and compares every queued frame and
finishes both Close handshakes.

Six sequential fresh four-worker release servers ran the cycle counts in order
64, 256, 256, 64, 64, 256. Rates and heartbeats were disabled, the write deadline
was 60 seconds, message queues allowed 1,024 entries, and the default 4 KiB read
and 32 MiB per-peer byte budgets remained in effect. Server-only RSS and metrics
were sampled about every 50 ms. All compilation and other socket workloads
finished before these measurements. The production compaction policy was
unchanged.

| Metric per run | 64 primers | 256 primers |
| --- | ---: | ---: |
| Large unknown-target input, approximately | 256 MiB | 1 GiB |
| Median sampled peak server RSS | 15,828 KiB | 15,920 KiB |
| Sampled peak RSS range | 15,696–15,872 KiB | 15,900–15,932 KiB |
| Held logical output bytes | 6,292,291 | 6,294,596 |
| Encoded value bytes reused | 8,388,648 | 8,388,648 |
| Encoded value bytes compacted | 211 | 981 |
| Verified data/barrier deliveries | 137 | 521 |

All six cases passed delivery/order checks, both Close handshakes, exact
incoming/delivery/value-byte counter totals, and cleanup to zero connections,
sessions, queued bytes, and variables with only the global channel remaining.
Protocol, transport, write, Close, overflow, and global-pressure error counters
were zero. The large unknown-target values produce no output reservations or
routing-byte counts. Compaction counts include the tiny queued values and
self-message barriers; large forwarded bodies retain shared storage.

Quadrupling the primer count changed the median sampled peak by 92 KiB in this
workload. This supports retaining the existing sharing/compaction policy. It is
a finite retention check, not a leak proof or a production capacity estimate.
Post-cleanup RSS ranged from 5.63 to 11.55 MiB; these observations do not identify
which allocator/runtime allocations account for that remaining storage. Logical
queue bytes and process RSS measure different things.

Reproduce with the commands in the README. Raw phases, samples, source and
executable hashes are in
[retention-pressure.json](benchmarks/2026-10-15-retention-pressure.json).
The recorded revision is the parent checkout before committing; source hashes
identify the measured implementation.

## Concurrent producers, connected idle storage, and retirement

The extended retention client starts synchronized producers feeding one unread
recipient. Each producer's sequence is checked independently; the relay does
not promise a cross-producer total order. The same 4 MiB unknown-target primers,
tiny private values, and self-message barriers exercise enlarged receive storage
while six MiB of initial output remains blocked. After complete drain, the client
observes open connections for two seconds, finishes client-initiated or expected
server-idle Close handshakes, then observes retirement for another two seconds.

Six initial cases used 64 cycles per producer, in order 1, 8, 32, 32, 8, 1.
They retained the default 32 MiB per-peer byte budget and used
`max(1024, producers * cycles + 32)` queue entries. Rates/heartbeats were disabled
and writes had a 60-second deadline. With application-idle expiry disabled, all
sessions remained joined during the connected-idle observation:

| Producers | Primer payload per run | Connected-idle RSS range | Tiny value bytes compacted |
| --- | ---: | ---: | ---: |
| 1 | 256 MiB | 15,716–15,720 KiB | 211 |
| 8 | 2 GiB | 38,592–44,644 KiB | 1,681 |
| 32 | 8 GiB | 137,476–143,540 KiB | 6,721 |

Each case reused 8,388,648 encoded large-value bytes. Empty output queues did not
return open connections to their initial read capacity. RSS is not an exact
allocation trace, but source inspection and the retirement observations support
budgeting enlarged input capacity separately from output reservations. The
configured 4 KiB read size is an initial size; incoming-size and admission limits
remain important for active populations.

The same six cases with a three-second application idle policy passed every
delivery, ordering, counter, and cleanup check. They observed exactly 2, 9, or 33
idle decisions and clean server-initiated handshakes. After the scheduling change
described below, final 1- and 32-producer reruns also passed. A further final case
used 64 producers, 32 cycles each, and 65 transports: all 4,105 data/barrier
deliveries and all 65 idle Close handshakes verified. It reused 8,388,648 encoded
large-value bytes and compacted 5,249 tiny bytes. Sampled RSS was 269,212 KiB
before acknowledgement and 6,560 KiB after retirement. All fifteen cases returned
to zero connections, sessions, queued bytes, and variables with the global
channel remaining; unexpected transport, write, Close, overflow, and pressure
counters were zero.

These finite observations are not a leak proof or a physical-memory cap. Published
shared bodies can outlive their source transport until recipients finish/discard
them. Application-idle retirement releases inactive sessions; active populations
still need input-size/admission budgeting and process RSS monitoring. See the
README and operations guide for the optional policy, disabled by default.

Raw records: [concurrent-retention.json](benchmarks/2026-10-15-concurrent-retention.json),
[concurrent-idle-expiry.json](benchmarks/2026-10-15-concurrent-idle-expiry.json),
[idle-final-retention.json](benchmarks/2026-10-15-idle-final-retention.json), and
[idle-final-64-producers.json](benchmarks/2026-10-15-idle-final-64-producers.json).
The first two identify the `c39d90e` implementation; the latter two identify the
final executable by hash. Parent revision fields predate their commits.

## Application-idle scheduling cost

The first idle-policy implementation reused one heartbeat/idle deadline wait,
but reset it whenever application activity moved the idle deadline later. Three
rotating twenty-second cases per configuration reproduced a cost relative to
the new disabled default. Separate user-cycle profiles attributed 8.38% of the
enabled profile to `tokio::time::sleep::Sleep::reset`.

The final implementation keeps an already earlier alarm when activity postpones
the deadline. When that alarm completes, it checks the current absolute deadline
and rearms before another poll; an earlier heartbeat deadline still retimes the
wait immediately. This preserves the deadline without a coarse periodic idle
check or per-message retiming. Application payload ownership and vectored writes
are unchanged. The final enabled profile no longer listed `Sleep::reset` among
symbols at or above the 0.5% reporting threshold.

The final comparison used four rotating twenty-second runs per candidate: fresh
four-worker servers, the same frozen 64-client/32-private-pair workload, 1 KiB
values, window 16, server CPUs 0–3, and client CPUs 4–7. Every candidate occupied
each run position once. Rates were disabled and heartbeats enabled, so each run
crossed the default first heartbeat. No compilation or profiling overlapped the
comparison, and relay workloads ran sequentially.

| Configuration | Median verified deliveries/s | Median server CPU ns/delivery | p99 histogram upper bound |
| --- | ---: | ---: | ---: |
| `14b9529` control | 1,610,854 | 1,780 | 1.049 ms |
| Initial enabled policy (`c39d90e`, 300 s) | 1,583,148 | 1,812 | 1.049 ms |
| Final, idle policy disabled | 1,701,361 | 1,689 | 1.049 ms |
| Final, idle policy enabled (300 s) | 1,606,449 | 1,776 | 1.049 ms |

All sixteen cases passed full frame verification and cleanup; routed copy counts
were zero. The same frozen private-traffic client drops its 64 socket handles at
the end instead of performing Close handshakes, so every comparison case records
64 expected transport disconnects. No protocol/write/Close-handshake error was
recorded. These comparisons establish data throughput and cleanup; graceful
Close acceptance comes from the separate retention, Rust-network, and JS cases.
CPU totals include that identical setup/teardown in every candidate.

Delayed rearming reduced median CPU per delivery by 2.0% and raised
median throughput by 1.5% versus the initial enabled implementation. Those
throughput ranges overlap; the modest local change is not a general speed claim.
The final disabled default measured 5.6% higher throughput and 5.1% less CPU per
delivery than the control. Enabling activity tracking still cost 5.6% throughput
and 5.2% CPU versus that new disabled default; it stayed close to the original
control's throughput/CPU with the same p99 bucket. Raw run ranges and positional
variation are retained rather than attributing every difference to one function.

The initial comparison/profile are
[application-idle-cost.json](benchmarks/2026-10-15-application-idle-cost.json) and
[application-idle-profile.json](benchmarks/2026-10-15-application-idle-profile.json).
Final records are [idle-final-cost.json](benchmarks/2026-10-15-idle-final-cost.json)
and [idle-final-profile.json](benchmarks/2026-10-15-idle-final-profile.json).
Profiles used `perf record`, user-space cycles at 199 Hz, DWARF call graphs,
twelve-second recording windows, and ten seconds of the same verifying traffic.
They were collected separately from timing; their percentages are sampled user
cycles, not shares of total server CPU. Recorder logs are preserved, and raw
`perf.data`/text reports remain local under `artifacts/profiles/`.
## Active decoder storage baseline, 2026-10-15

Fresh frozen-server measurements reproduced persistent input storage with empty
queues and application-idle expiry disabled. After 64 four-MiB unknown-target
primers per source, 32 still-connected producers retained roughly 134 MiB RSS;
closing the sockets reduced it to roughly 6 MiB. One- and eight-producer cases
also passed all delivery, sequence, Close, counter and cleanup checks. The raw
records are `benchmarks/2026-10-15-active-input-baseline.json` and
`benchmarks/2026-10-15-active-input-baseline-phases.json`. These are observed RSS
values, not an accounting of exact live allocations or an input-memory cap.

The retention client now emits `closing` before initiating its Close handshakes.
The sampler drains phase events before and after reading metrics/process state,
labels observations spanning a phase change as `transition`, and excludes those
observations from stable holding/idle/retired ranges. This fixes a reproduced
false failure in an experimental run, where already-closing sockets were still
classified as connected idle. It retains the full observations and phase names
in the record. Repeated `--server-arg=ARG` options now support explicit policy
comparisons; the complete server command remains in each case.

## Retiring unused decoder storage, 2026-10-15

The native relay now schedules receive-storage reclamation after five seconds
without a completed binary message larger than `max(64 KiB, initial read size)`.
It shares the existing pinned heartbeat/idle deadline wait. Large recurring input
postpones the operation, preserving buffer reuse during a burst. Small-message
traffic does not read an extra clock for this policy. Zero disables it.

The decoder exposes one narrow safe operation through `tungstenite` and its
Tokio wrapper. It replaces unused input storage only between frames and only
after the normal parser proves any remaining bytes are an incomplete header.
It can copy at most thirteen header bytes; it never copies a published body.
Busy input defers the attempt. Fragment assembly and control state stay in the
existing decoder. Successful reclamation also lowers the connection's routing
watermark so later tiny values can share its normal slab again. The complete
three-file patch, original package checksums/commits, license attribution, and a
file-by-file source audit are in [vendor](../vendor/README.md).

An exploratory eager reset at each empty read increased allocation churn and
allocator-retained RSS for small populations. It was discarded. The delayed
policy keeps reuse during the measured primer bursts and retires the unused
storage afterward. It is a memory-lifetime improvement with measured tradeoffs,
rather than a claim that dropping an allocation always returns its pages.

### Matched connected-idle observations

The control and candidate use the same frozen verifying retention client,
64 cycles of four-MiB unknown-target primers per producer, eight one-MiB output
fillers, and a six-second idle observation after all deliveries drain. Each case
uses a fresh four-worker process, disabled heartbeat/rates/application-idle Close,
the same 4 KiB initial read setting, and the existing output budgets. Producer
order varies across repetitions. Source/binary hashes are in
[read-buffer-idle-build.json](benchmarks/2026-10-15-read-buffer-idle-build.json).

This table uses each run's **last connected-idle sample**, with all connections
still present and queues empty. Values are medians of three runs per binary;
complete ranges and every observation remain in the
[summary](benchmarks/2026-10-15-read-buffer-idle-summary.json) and its referenced
control/default retention records.

| Producers, plus one receiver | Control RSS | New default RSS | Change |
| --- | ---: | ---: | ---: |
| 1 | 9.55 MiB | 11.41 MiB | No stable RSS gain; ranges overlap and allocator variation dominates |
| 8 | 37.76 MiB | 11.57 MiB | 69.4% lower |
| 32 | 140.24 MiB | 12.02 MiB | 91.4% lower |

All eighteen cases passed exact data/order, backpressure, counter, Close and
cleanup checks. Each enabled run completed exactly one reset per producer,
copied zero wire-header bytes, and reported zero deferred resets. At one
producer, the control range was 9.54–15.36 MiB and the candidate range was
5.52–11.42 MiB; the higher candidate median is retained explicitly. Freed
storage and process RSS are different quantities.

A separate disabled-policy 32-producer run retained 143,592 KiB with zero
resets, consistent with the control. A 64-producer/65-connection pair, using
32 cycles each to cap primer payload at eight GiB, passed all 4,105 data/barrier
deliveries and all 65 clean Close handshakes. The enabled process's last
connected-idle sample was 12,732 KiB versus 269,236 KiB for the control; all
65 sockets remained connected. Sampled peak RSS was effectively unchanged at about
275,284/275,292 KiB. Reclamation acts after input is no longer needed; it does
not reduce the demonstrated peak needed to process concurrent large frames.
Those individual records are [default at 64 producers](benchmarks/2026-10-15-read-buffer-idle-default-64-producers.json),
[control at 64 producers](benchmarks/2026-10-15-read-buffer-idle-control-64-producers.json),
and [disabled at 32 producers](benchmarks/2026-10-15-read-buffer-idle-disabled-32-producers.json).

### Traffic cost and ordinary idle connections

The same frozen traffic generator ran 64 clients as 32 private pairs, window 16,
four server workers on CPUs 0–3 and the client on CPUs 4–7. Candidate order
rotated; timed runs did not overlap compilation, profiles, or other workloads.
Each received frame was verified. The following are process-run medians:

| Payload | Binary/configuration | Deliveries/s | Server CPU ns/delivery | p99 bucket upper bound |
| --- | --- | ---: | ---: | ---: |
| 1 KiB | Control | 1,727,933 | 1,687 | 1.049 ms |
| 1 KiB | New default | 1,732,999 | 1,683 | 1.049 ms |
| 1 KiB | New binary, policy disabled | 1,727,633 | 1,687 | 1.049 ms |
| 64 KiB | Control | 205,060 | 11,917 | 4.194 ms |
| 64 KiB | New default | 198,605 | 12,226 | 4.194 ms |
| 64 KiB | New binary, policy disabled | 199,938 | 12,226 | 4.194 ms |

All twelve twenty-second 1 KiB runs and nine fifteen-second 64 KiB runs passed.
Throughput ranges overlap. No general CPU/throughput gain is claimed. The 64 KiB
candidate measured 2.6% more CPU per delivery and 3.1% less median throughput
than the control. The disabled new binary showed a similar CPU cost, so these
measurements do not isolate that difference to active reclamation. Some early
large-message cases occupied the 8.389 ms p99 bucket; the table retains the
median bucket and the raw per-run variation remains available.

Body-compaction counters stayed zero in every traffic case. These continuous
workloads keep postponing the storage deadline; actual resets and correctness
across them are established separately by the retention and socket regressions.
The frozen traffic generator deliberately drops its 64 socket handles after
timing. Every run recorded 64 combined abrupt read/write departures; one large
control run recorded 63 read-side disconnects plus one write error, first visible
after timed samples ended. Protocol errors, write timeouts and Close failures
were zero, and all resource gauges cleaned up. This traffic teardown is not a
graceful-Close acceptance check.

Raw records and full traces are
[small traffic](benchmarks/2026-10-15-read-buffer-idle-cost.json) and
[large traffic](benchmarks/2026-10-15-read-buffer-idle-large-cost.json).
Three alternating 2,000-socket idle trials per binary, with one fixed load
client, measured median server RSS increases of 22,692 KiB for the control and
22,712 KiB for the candidate: 20 KiB more. No storage reset occurred because
these clients sent no large input. This is a two-second idle-state measurement,
not a high-active-population or production-capacity claim. The raw record is
[small connections](benchmarks/2026-10-15-read-buffer-idle-small-connections.json).

### Scope and final acceptance

The public-API regressions verify release of the decoder's shared owner without
moving the published payload, all fourteen masked header splits, deferred partial
payloads and a buffered small frame. Native socket regressions verify continued
routing without later tiny-body compaction, a pending frame across the deadline,
and fragmented-message assembly with Ping/Pong across reclamation. All 52 Rust
checks, strict Clippy, seven real-socket JS cases, the new 61-file codec source
audit, and the original C++ comparison passed locally. The C++ comparison still
passes 46 common cases plus its eight explicit differences.

The refreshed local release actually reclaimed its decoder storage after an
observed 5.008 seconds while both clients stayed connected. A subsequent typed
private value preserved bytes and an exact uint64 with zero additional routed
body copies. Both clients completed code-1000 closes. A fresh Chrome run passed
seven bundled-client checks, and three live legacy/Close checks passed. Health
and readiness returned 200, and final connection/session/queue/variable gauges
and error counters were clean. Exact process hashes and evidence are in
[read-buffer-idle-live.json](benchmarks/2026-10-15-read-buffer-idle-live.json).

This releases unused decoder ownership. It does not impose a process-wide input
allocation budget, free incomplete messages, or guarantee the allocator returns
pages. The following iteration adds reservations before decoder growth;
connection/input limits and actual RSS still matter.

## Admission before decoder growth

The next measured problem was simultaneous incomplete input. The decoder reserves
the advertised frame length before the body finishes arriving, and fragmented
messages have a separate assembly buffer. Delayed cache reclamation cannot
prevent those allocations. A 64-source baseline advertised 8 MiB frames and
supplied only 2 MiB to each; all were admitted. Three fresh processes held
148–149 MiB RSS while an independent healthy session continued to echo data.

The selected implementation adds a shared **1 GiB staging allowance**, adjustable
with `--total-read-buffer-bytes` (`0` disables). A connection reserves before its
bounded HTTP upgrade; frame and fragment growth then acquire separate high-water
leases before allocating. Twice the requested capacity covers old/new staging
overlap. Large frame capacity includes one configured read chunk of read-ahead.
Ordinary reads within the high water perform no shared-budget atomics or new
guard allocations. Reclamation shrinks unused allowances; transport drop or
cancellation releases them. Published `Bytes` keep their original owners and
pointers and require separate consumer budgets. This is not an RSS cap.

An exploratory exact-capacity implementation passed correctness but increased
64 KiB server CPU per delivery by 9.1% in the first three-way comparison. A
userspace cycle profile identified the reader as a hot path; source inspection
suggested exact frame boundaries lost reusable storage for the next header.
Keeping one bounded read chunk recovered the cost in a second whole-socket
comparison. This is a source-supported explanation, not a syscall/allocation
count measurement. Both experimental binaries and all raw records are retained.

The second comparison rotated three candidates across all positions, three
fresh-process repetitions each, 15 seconds per run, 64 clients forming 32 private
pairs, a window of 16, and 64 KiB payloads. Four server workers used CPUs 0–3;
the fixed verifying client used CPUs 4–7. Rates were disabled and heartbeats
enabled. CPU includes setup/teardown; latency is a fixed-histogram upper bound.

| 64 KiB candidate | Median verified deliveries/s | Median server ns/delivery | Median p99 upper bound |
| --- | ---: | ---: | ---: |
| Prior server (`bf8d2e9`) | 201,967 | 12,131 | 4.194304 ms |
| Exploratory exact capacity | 206,765 | 13,593 | 4.194304 ms |
| Selected charged read-ahead | 205,480 | 11,867 | 4.194304 ms |

The selected version used 12.7% less CPU per delivery than exact capacity and
2.2% less than the prior server, with 1.7% higher throughput than the prior
server. These are small local differences against the prior server, not a
general speed guarantee. The reason for admission is bounded staging growth.
Every frame was verified, routed-value compaction stayed zero, and the selected
and exploratory final staging reservations were zero in every run. The frozen
traffic client intentionally drops its 64 socket handles after timing: these
transport departures do not establish graceful-Close acceptance.

Sources, release hashes, original package checksums and the complete codec patch
are bound in [input-budget-build.json](benchmarks/2026-10-15-input-budget-build.json).
The [initial 64 KiB comparison](benchmarks/2026-10-15-input-budget-traffic-64k.json),
[reader profile](benchmarks/2026-10-15-input-budget-profile.json) and
[selected comparison](benchmarks/2026-10-15-input-budget-read-ahead-64k.json)
include raw evidence or links to their complete traces. Original package
verification covers all 62 vendored files plus the declared patch.

The final small-message comparison used the same fixed client, affinity, window,
duration and alternating candidate order, three repetitions per server. At 1 KiB,
median throughput was 1,685,370/s before and 1,698,247/s after admission; server
CPU was 1,716 versus 1,690 ns per verified delivery, with the same 1.048576 ms
p99 bound. These sub-2% differences support retaining the bounded policy without
a broad throughput claim. Every delivered value was checked, compaction stayed
zero and all final staging/queue/session gauges were zero. Full traces are in
[the selected 1 KiB comparison](benchmarks/2026-10-15-input-budget-read-ahead-1k.json).

Admission has a standing-memory cost. Three alternating fresh-process pairs with
2,000 idle joined sockets measured median RSS deltas of 22,704 KiB before and
23,644 KiB after: **940 KiB more**, approximately 0.46 KiB per session on this
host. Staging reservations were exactly 16,384,000 bytes with no refusals. These
idle samples verify the active population; the idle runner does not separately
sample final cleanup.

Three alternating ten-second churn pairs used 64 workers and the same fixed
verifying client. Each lifecycle joins, verifies one self-routed counter and its
Close handshake. Every run passed with zero final staging/queue/session/connection
gauges and no protocol, handshake, write, Close or admission errors. Churn rate
ranges overlap substantially, so they establish correct sustained admission and
cleanup rather than a stable speed improvement. The full populations, CPU and
memory records are in [input-budget-resources.json](benchmarks/2026-10-15-input-budget-resources.json).

Nine final incomplete-input cases used the selected release and the same frozen
client. Three alternating control/limited pairs joined 64 source channels plus
one independent healthy client, advertised masked 8 MiB frames, supplied only
2 MiB per admitted source, and held for two seconds. The healthy client verified
1 KiB self echoes throughout. Admitted incomplete sockets intentionally dropped
TCP; two ordinary sessions verified normal Close and a new join/route after
the reservations released. All connection/session/queue/variable/staging gauges
returned to zero, with the persistent global channel remaining.

| Incomplete-input configuration | Admitted / refused sources | Held RSS |
| --- | ---: | ---: |
| Prior server, three runs | 64 / 0 | Median 148.22 MiB; range 143.87–156.38 MiB |
| Selected, 24 MiB staging allowance, three runs | 1 / 63 (`1013`) | Median 8.14 MiB; range 8.12–8.17 MiB |
| Selected default 1 GiB allowance, one run | 63 / 1 (`1013`) | 151.94 MiB |
| Selected admission disabled, one run | 64 / 0 | 149.81 MiB |

The tight allowance reduced median held RSS by **94.5% in this configured
workload**. The default admits most of this cohort and does not establish a
general RSS reduction; staging headroom and RSS measure different quantities.
An additional single-source case filled its exact 16,793,600-byte allowance,
kept the independent healthy client routing, verified HTTP `503` for another
handshake, and admitted a new client after release. All nine cases verified
post-release routing, two normal Close handshakes, exact expected incomplete
TCP-drop counts and zero protocol/write/Close errors. Refused incomplete frames
verify the received Close code, not a full acknowledgement exchange.

The selected release also passed an eight-producer retention/reclamation check
with 64 cycles of 4 MiB primers per producer. Every ordered data item/barrier and
all nine Close handshakes were verified. Last connected-idle RSS was about
11.66 MiB after the default five-second reset; final staging allowance was zero.
This retains shared application ownership while separating admission from cache
retirement. The [summary](benchmarks/2026-10-15-input-budget-summary.json) binds
every pressure source record and its hash; the
[retention record](benchmarks/2026-10-15-input-budget-retention.json) includes all
phases and final metrics. These are finite plaintext loopback checks, not a
production capacity or multi-hour stability claim.

The selected native and bridge releases also passed the 46-common-case/eight-
explicit-difference [original C++ comparison](benchmarks/2026-10-15-input-budget-reference.json),
all 60 Rust checks and seven JavaScript socket cases. After a subsequent host
reboot, runtime source/release hashes still matched their saved manifest. The
local services and headed Chrome were restarted, and Rust/Clippy/format/vendor
checks, seven Chrome checks, three live legacy/Close checks and a shared-routing
reclamation check passed again. The latter shrank staging from 8,405,022 to
16,384 bytes after 5.022 seconds while both sessions remained connected;
subsequent typed routing added no body copies. Both completed normal Close and
final staging/connection/session/queue/variable gauges were zero. Health and
readiness returned 200. [Live evidence](benchmarks/2026-10-15-input-budget-live.json)
records exact process hashes, the browser report, and the post-reboot boundary;
performance results were collected before the reboot.

## Fragment assembly capacity and repeated pressure, 2026-10-16

The next real-socket regression was a complete 4 MiB CBOR message split into a
3 MiB first fragment and a final 1 MiB continuation. Under a 16 MiB staging
allowance, the previous release (`8ab12a0`) returned `1013 Input Memory Limit`.
The frame storage and necessary 4 MiB assembly fit; geometric assembly growth
requested 6 MiB, which made the conservative twice-capacity allowances exceed
the budget. The failure also occurred with the normal 16 MiB message ceiling.
The [control record](benchmarks/2026-10-16-fragment-final-control-tight.json)
expects and verifies that refusal; its passing status describes the reproduction,
not acceptance of a valid message.

Budgeted assembly now bounds intermediate geometric growth by the configured
message ceiling. When a final fragment needs growth, it allocates the now-known
complete length. Thus the example grows assembly to 4 MiB, including with a
larger configured ceiling. Existing sufficient capacity stays in place; the
decoder does not shrink or copy a completed payload. Actual-length validation
still precedes admission, and the unmetered path retains upstream allocation
behavior. Binary and text assembly use the same bounds, including tiny limits
and UTF-8 characters split between fragments. The
[fixed release check](benchmarks/2026-10-16-fragment-final-after.json) completes
the message with a peak staging allowance of 14,696,448 bytes and zero refusals.

Three alternating fresh-process pairs compared the prior and final servers with
the same frozen client binary. Eight sources each sent three verified 4 MiB
messages, each as masked 3 MiB/1 MiB fragments, under a 256 MiB staging allowance
and a 16 MiB message ceiling. Interleaved Ping/Pong proved first-fragment
assembly before completion. An independent session verified 1 KiB echoes every
20 ms, including during assembly and retirement. Each wave waited two seconds
for the explicitly configured one-second decoder reset, then a fresh client
joined, self-routed and completed normal Close. Four server workers used
plaintext loopback; runs were sequential, without concurrent compilation.

| Eight-source, three-wave result | Prior release | Final release |
| --- | ---: | ---: |
| Verified fragmented messages per run | 24 | 24 |
| Peak staging allowance, all three runs | 144.0703 MiB | 112.0703 MiB |
| Median sampled peak server RSS | 90.08 MiB | 77.50 MiB |
| Sampled peak RSS range | 79.39–90.13 MiB | 62.36–81.46 MiB |
| Connected allowance after every reset | 72 KiB | 72 KiB |
| Verified normal Close handshakes per run | 12 | 12 |

The allowance reduction is exactly **32 MiB, or 22.2%**, for this workload.
Median sampled peak RSS was 14.0% lower, with substantial host/allocator
variation; RSS was sampled about every 50 ms and is a separate measure from
staging allowance. Healthy maximum round trips ranged from 0.30 to 1.07 ms
across both candidates. These short spaced-message runs establish verified
concurrent progress and storage behavior, not a CPU, throughput or production
latency improvement. Every run returned final staging/connection/session/queue/
variable gauges to zero, retained only the persistent global channel, and had
zero protocol/write/Close/transport/admission errors. The encoded CBOR value was
checked byte for byte and routed-body compaction stayed zero. Required WebSocket
unmasking and fragment assembly are outside that routing-copy metric.

Additional final-release cases use the same 16 MiB message ceiling:

- **64 sources plus the healthy session**, three waves of 1 MiB messages:
  all 192 messages completed. Each wave returned to 532,480 bytes of connected
  allowance (65 × 8 KiB); final allowance was zero.
- **Default five-second retirement**, two 4 MiB waves under a 16 MiB staging
  allowance: both completed, both resets occurred, and the two connected
  sessions returned to 16 KiB before final Close.
- **Retirement disabled**: all 24 messages completed and the expected
  117,514,240-byte high water remained between waves. There were no decoder
  resets; transport retirement released the entire allowance.
- **Admission disabled**: all 24 messages completed with allowance gauges zero;
  all 24 scheduled decoder resets still occurred.
- **64 KiB and 64 KiB + 1 byte**, split after one byte: both 16-message cases
  completed. The exact threshold caused zero resets; one byte above caused
  all 16, returning to the 72 KiB connected allowance after each wave.
- The separate **64-source incomplete-frame** check still admitted exactly one
  8 MiB advertised frame under a 24 MiB staging allowance and refused 63 with
  `1013`, while healthy routing and later admission stayed verified. Its admitted
  unfinished transport intentionally drops TCP; ordinary Close is verified
  separately.

The [final summary](benchmarks/2026-10-16-fragment-final-summary.json) binds every
final comparison and pressure trace by hash, including per-wave observations and
exact counters. The [build manifest](benchmarks/2026-10-16-fragment-final-build.json)
binds the source files, original package provenance, complete vendor patch,
release binaries and unchanged verifying client. Earlier `fragment-cap`,
`fragment-limit` and `fragment-wave` records preserve an exploratory change that
only capped growth at the configured ceiling. Its
[remaining below-ceiling failure](benchmarks/2026-10-16-fragment-final-before.json)
motivated passing the existing final-fragment flag to assembly; those earlier
records are not measurements of the final implementation.

Reproduce the workload with `cargo build --release --bins --example fragment_waves`
and `python3 scripts/fragment-waves.py --max-message-bytes 16777216 --output
artifacts/fragment-waves.json`. The benchmark overrides the reset interval to
one second; the ordinary server default remains five seconds. See the README
for the larger cohort and disabled-reset variants.
