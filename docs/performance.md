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

## Scheduling during empty-fragment input, 2026-10-16

Message size and staging admission do not bound the work done before a
fragmented message completes. A valid message can contain many empty masked
continuations. The decoder previously parsed until a complete message or actual
I/O blocking, while Tokio's I/O budget counted reads rather than frames. A
single ready socket could therefore delay unrelated routing and management
requests, especially with larger configured read chunks. This was reproduced
against `9ddd0ee` before changing the parser.

The decoder now exposes a bounded step using its existing parsing loop.
`read_with_frame_limit(NonZeroUsize)` returns `Ok(None)` when its frame quota is
exhausted without a message. The asynchronous wrapper explicitly wakes itself
and returns `Pending`; buffered input can then resume without waiting for a new
network event. Native and legacy server sockets enable **256 frames per poll**.
The library default and ordinary synchronous `read()` remain unlimited. No wire
validation, message/rate limit, payload owner or ordering changes. The hook
allocates no new payload buffer and carries incomplete UTF-8/control state and
admission leases across yields and cancelled read futures.

This provides scheduling opportunities rather than a time deadline. A single
frame can still require its normal body validation, unmasking and assembly;
the quota also does not cap a connection's total CPU use. Tokio's
[runtime fairness contract](https://docs.rs/tokio/latest/tokio/runtime/index.html#detailed-runtime-behavior)
requires bounded task polls and does not promise a particular scheduling order.
The measured latency below is specific to the workload and host.

Three alternating fresh-process pairs used one worker on CPU 0 and the identical
independent Python client on CPUs 4–7. Four source sockets each completed eight
small CBOR messages containing two million empty continuations apiece:
**64 million valid empty continuations and 32 verified messages per run**.
Each message's encoded value included a checked wave counter. A separate socket
verified 1 KiB self echoes about every millisecond, and management latency was
sampled approximately every 10 ms. All five sockets completed normal Close.
Rates, heartbeats and application idle were disabled; message/staging defaults
remained enabled. Preparation preceded timing; no compilation or competing
benchmark ran alongside a measured case.

| Read chunk / observation | Prior release | Bounded parsing |
| --- | ---: | ---: |
| 4 KiB: median of each run's maximum healthy RTT | 10.662 ms | 0.871 ms |
| 4 KiB: range of maximum healthy RTTs | 9.471–11.140 ms | 0.831–0.910 ms |
| 4 KiB: median of maximum management RTTs | 18.059 ms | 1.583 ms |
| 128 KiB: median of maximum healthy RTTs | 322.164 ms | 0.864 ms |
| 128 KiB: range of maximum healthy RTTs | 215.464–324.853 ms | 0.859–1.067 ms |
| 128 KiB: median of maximum management RTTs | 364.034 ms | 1.468 ms |

With 4 KiB reads the prior server completed 272–301 healthy probes during each
burst; bounded parsing completed 1,395–1,401. With 128 KiB reads those populations
were 20–24 versus 1,390–1,417. The prior larger-buffer runs have too few healthy
observations for a robust percentile claim, so the table reports per-run maxima.
Median server CPU increased from 1.73 to 1.76 seconds with 4 KiB reads and from
1.70 to 1.73 seconds with 128 KiB reads, about **1.7–1.8% more**. CPU is sampled
from `/proc` at 10 ms resolution. This is a measured scheduling/CPU tradeoff for
fragment-heavy input, not a throughput improvement.

Every fragment workload verified exact incoming message/byte totals, join and
delivery counts, delivered byte totals, normal Close and final resource release.
Staging reservations, sessions, connections, queued and variable bytes returned
to zero with the persistent global channel remaining. Protocol, write, timeout,
Close, transport and admission errors were zero, and routed-body compaction was
zero. The three new Rust checks additionally prove self-wakeup with entirely
buffered input, state preservation when pending read futures are dropped,
split UTF-8 with interleaved Ping/Pong, and terminal protocol-error behavior.

A separate four-worker pair on CPUs 0–3 used 16 sources, eight messages each and
two million empty continuations per message (256 million continuations per run).
Maximum healthy RTT was 9.230 versus 0.499 ms with 4 KiB reads, and 294.970 versus
0.797 ms with 128 KiB reads. All 128 messages and 17 normal Close handshakes were
verified in each case. These are single pairs supporting the repeated
single-worker finding, not a larger-population or production latency bound.

Ordinary complete-message cost was checked with three alternating 15-second
pairs per payload, four server workers on CPUs 0–3, and the same frozen Rust
traffic client on CPUs 4–7. Sixty-four peers formed 32 private pairs with a
window of 16; every encoded value and its order were verified.

| Complete-message workload | Prior deliveries/s | Bounded deliveries/s | Prior server ns/delivery | Bounded server ns/delivery |
| --- | ---: | ---: | ---: | ---: |
| 1 KiB, medians | 1,777,326 | 1,779,085 | 1,605 | 1,614 |
| 64 KiB, medians | 211,040 | 208,921 | 11,407 | 11,426 |

The 1 KiB CPU median increased 0.53% and the 64 KiB median increased 0.17%;
throughput changed +0.10% and −1.00%, within overlapping ranges. The p99 upper
bounds remained 1.048576 and 4.194304 ms respectively. These results support
retaining the scheduling change without claiming a general traffic speed gain.
All final resource gauges were zero, compaction stayed zero, and the frozen
client's 64 intentional TCP departures were accounted for in every run.

The final release also passed two 4 MiB fragmented waves under a tight 16 MiB
staging allowance and the ordinary five-second reset, plus 64-producer/one-healthy
session reclamation across three 1 MiB waves. All 192 larger-cohort messages,
68 normal Close handshakes and three returns to the expected 532,480-byte
connected allowance were verified. The original C++ comparison now additionally
checks binary and split-UTF-8 text with 1,024 empty continuations: all 48 common
cases and eight explicit differences passed. Those added cases exercise the
legacy server's quota as well as preserving the original wire behavior.

The [build manifest](benchmarks/2026-10-16-fairness-build.json) binds original
package provenance, the full codec patch, source/release/client hashes, exact
verifier source and test evidence. The
[summary](benchmarks/2026-10-16-fairness-summary.json) binds all comparison traces
and ordinary traffic checks. The traffic verifier intentionally drops its socket
handles at timing end; those records account for every expected TCP departure
and establish delivery/ordering and cleanup. Normal Close is verified separately
by the fragment workload and compatibility checks. An initial driver preflight
incorrectly expected graceful teardown from that frozen traffic client; its
record and correction are retained.

Reproduce the finite probe with `python3 scripts/fragment-fairness.py --workers 1
--output artifacts/fragment-fairness.json`. Its default is 500,000 empty
continuations per message; the comparison used `--fragments 2000000 --cycles 8`.
Affinity is optional and requires an appropriate Linux CPU set. These checks
do not establish production latency, multi-hour stability or cluster capacity.

## Cost check for pre-join rejection reporting, 2026-10-16

Native pre-join decoder failures now share the joined-session Close mapping and
increment `war_join_rejected_total` once per rejection. The counter is global,
with no labels or per-connection storage; successful packet processing has no
new counter update. Body validation and shared forwarding storage are unchanged.
The behavioral reproduction and independent corpus are described in
[stability.md](stability.md#rejections-before-join-and-mixed-input-2026-10-16).

Three alternating fresh-process pairs per payload checked ordinary traffic
against `9f39159`: four server workers on CPUs 0–3, the same frozen verifying
client on CPUs 4–7, 64 peers/32 pairs, window 16, ten seconds per run. Every value
and order were checked. Server CPU includes setup and teardown; latency
percentiles are fixed-histogram upper bounds.

| Payload / median | Prior release | Rejection reporting |
| --- | ---: | ---: |
| 1 KiB deliveries/s | 1,789,088 | 1,777,778 |
| 1 KiB server ns/delivery | 1,593 | 1,597 |
| 64 KiB deliveries/s | 199,861 | 199,851 |
| 64 KiB server ns/delivery | 11,766 | 11,623 |

Throughput medians changed −0.63% and −0.005%; CPU changed +0.21% and −1.21%.
The 1 KiB p99 bound stayed 1.048576 ms. The 64 KiB bounds ranged from 4.194304
to 8.388608 ms before and were 4.194304 ms after. These overlapping local runs
support retaining the diagnostic fix without a general speed or latency claim.
All twelve cases verified expected resource release and the frozen client's
64 intentional TCP departures per run, with zero new join rejections, protocol,
timeout, Close or admission errors and zero routed-body compaction. They do not
verify graceful Close; the input corpus and compatibility cases do so separately.

The selected release also passed the existing 64-source incomplete-frame check
under a 24 MiB staging allowance: one source was admitted, 63 received `1013`,
healthy routing and later admission remained verified, and all final staging/
queue/session/connection/variable gauges were zero. Its admitted incomplete
transport intentionally drops TCP. The
[summary](benchmarks/2026-10-16-input-rejection-summary.json) binds the corpus,
traffic, pressure and acceptance records; the
[manifest](benchmarks/2026-10-16-input-rejection-build.json) binds source, package,
patch and fixed-client hashes. This is finite plaintext loopback evidence.

## Cost check for optional browser-origin admission, 2026-10-16

The shared policy borrows configured strings and parsed HTTP header bytes once
per upgrade. There is no URL parsing or allocation in its acceptance check and
no new per-message work. An empty list preserves unrestricted admission; an
enabled list compares at most 64 exact origins. Denial builds a small fixed HTTP
response and updates native rejection counters. Native application-body storage,
queue ownership and vectored writes are unchanged. The optional bridge also
reuses the existing bounded HTTP reader; its post-upgrade translation path and
16 MiB data-message ceiling remain intact.

All comparisons use the same newly frozen verifying client, sending the same
`Origin: https://app.example` to both the prior `1c3a603` release and the new
release. Fresh four-worker relays run sequentially on CPUs 0–3, with the client
on CPUs 4–7. Each churn iteration joins, verifies its monotonic self-message and
finishes Close before reconnecting. Its latency measures connection through
verified self-delivery; Close follows that timing interval. CPU includes setup
and teardown, sampled at the host's 10 ms tick resolution. Percentiles are fixed
histogram upper bounds. Incoming rate limits are disabled for the workload;
the incoming-message ceiling is 1 MiB. Heartbeats, the 4 KiB initial read buffer,
five-second decoder retirement and 1 GiB staging admission retain their defaults.

Three five-second repetitions per arm measured 64 independent churn workers.
The 64-entry list places the match last, exercising the bounded linear scan:

| Five-second churn / median | Lifecycles/s | Server ns/lifecycle | p99 upper ms |
| --- | ---: | ---: | ---: |
| Prior release | 110,642 | 17,301 | 2.097152 |
| New, unrestricted | 105,268 | 17,250 | 2.097152 |
| New, one allowed origin | 110,106 | 17,354 | 2.097152 |
| New, 64 allowed origins | 110,157 | 17,519 | 2.097152 |

The enabled one/64-entry arms used +0.31%/+1.26% median server CPU per lifecycle
against that control. The unrestricted arm's throughput median was 4.86% lower
despite similar CPU cost and overlapping ranges, so it received a longer check:
four ten-second repetitions per build, alternating order. Those medians were
108,275 versus 108,148 lifecycles/s (−0.12%) and 17,462 versus 17,479 server
ns/lifecycle (+0.09%), with the same 2.097152 ms p99 bound and overlapping ranges.
The initial spread remains in the record; it is not attributed to the policy.

Three four-second traffic pairs checked 64 peers/32 private-routing pairs,
1 KiB values and a 16-message window. Every complete encoded value and its
order were verified. Throughput medians were 1,719,050 versus 1,707,154
deliveries/s (−0.69%); server CPU was 1,627 versus 1,622 ns/delivery (−0.28%).
The p99 bound stayed 1.048576 ms. This is a cost guard, with no speed improvement
or production capacity claim.

All 26 cases passed exact join/departure counts and resource/error checks:
15,148,001 complete churn lifecycles and 41,517,392 ordinary traffic deliveries.
No application values were compacted in these workloads. Churn verifies Close;
the traffic generator intentionally drops 64 TCP handles at its deadline, and
each traffic case accounts for exactly those transport/write departures.
Sessions, connections, queued/variable bytes and decoder reservations end at
zero, leaving the persistent global channel. Unexpected rejection, protocol,
Close, timeout and pressure counters are zero.

The [short comparison](benchmarks/2026-10-16-origin-cost.json) and
[longer confirmation](benchmarks/2026-10-16-origin-churn-confirm.json) retain raw
samples, commands, client/release digests and per-case checks. The
[summary](benchmarks/2026-10-16-origin-summary.json) and
[manifest](benchmarks/2026-10-16-origin-build.json) bind them to source and live
acceptance. For a fresh enabled-policy workload, build the stress example and
run `python3 scripts/stability.py --modes churn --churn-secs 10 --origin
https://app.example --allowed-origin https://app.example --output
artifacts/origin-churn.json`. Other browser engines, TLS/proxies, adversarial
Origin admission rates, legacy CPU cost and larger/longer deployments remain
unmeasured.

## ID registry capacity after population waves, 2026-10-16

Two global clients routed more slowly after 8,192 peers in separate rooms had
joined and departed. An initial unchanged-release run measured 1,444 versus
1,686 server ns per verified delivery before/after the wave. The ID ownership
map retained its large table, and `values()` kept visiting empty buckets, as
described by the [standard-library iteration contract](https://doc.rust-lang.org/std/collections/struct.HashMap.html#method.iter).
The transient rooms were empty and removed; global membership stayed at two.
Thus this global broadcast path isolated the ID index's history from departure
fanout and room membership scans.

A separate `cycles:u`/DWARF profile attributed 23.97% of sampled user cycles to
`Relay::receive`. Its annotated SIMD empty-control-byte group loop accounted
for about 61.4% of that function, or 14.7% of sampled user cycles. The source
path, workload and instructions identify the ID-map scan. The selected profile
reduces `Relay::receive` to 7.91%. Each profile has 2,250 samples and no lost
samples. Profiled timings are diagnostic and excluded from timing comparisons;
the reports and annotations are retained with digests of the local binary data.

The shared retirement helper checks only maps above 256 capacity and no more
than one-eighth occupied, then shrinks with room for twice the remaining IDs.
This hysteresis keeps ordinary small churn from allocating replacement tables.
It runs under the existing write lock after session removal or occupied-ID
takeover. It moves keys and `Arc` handles; peers, queues and shared application
bodies retain their addresses. There is no additional per-message check,
allocation, body movement, timer, background sweep or operator setting.

The departure-only prototype recovered routing cost but missed takeover-driven
retirement while displaced registrations were still alive. A failing 1,024-owner
regression demonstrated the gap, and the selected helper covers both paths.
Prototype records and its frozen source/binary remain distinct from the final
comparison. The [stability checks](stability.md#id-registry-lifetime-across-large-populations-2026-10-16)
cover delayed cleanup, stable payload pointers and healthy concurrent routing.

The final comparison uses three fresh processes per build, reversing order on
the second repetition. Relays use four workers on CPUs 0–3, with one frozen
client on CPUs 4–7. The same two global clients stay connected through three
8,192-peer waves per process. Transient peers use distinct ordinary rooms and
verify exact normal Close and EOF. Three-second traffic phases precede the first
wave and follow every wave: 64 prebuilt 1 KiB values, 16-message windows, every
encoded sender/value and FIFO position checked. Stdin gates separate filling,
draining and traffic; native CPU includes boundary management sampling at the
host's 10 ms tick resolution. Incoming rates and heartbeats are disabled in this
probe, with default message/storage admission policies retained.

Post-wave values below are medians of each process's three-wave median, then
medians across its three processes. Fresh phases remain separate:

| Median / two global clients | Prior `241a172` | Selected |
| --- | ---: | ---: |
| Fresh deliveries/s | 449,174 | 446,403 |
| Fresh server ns/delivery | 1,440 | 1,449 |
| After waves, deliveries/s | 416,192 | 447,340 |
| After waves, server ns/delivery | 1,638 | 1,464 |
| After waves, p99 upper bucket | 0.032768 ms | 0.032768 ms |
| Fill one 8,192-peer wave, server CPU | 0.16 s | 0.16 s |
| Drain one wave, server CPU | 0.09 s | 0.09 s |

The selected post-wave workload uses **10.64% less server CPU per delivery**
with **7.48% higher throughput**. Fresh CPU differs by +0.62%. Repeated regrowth
and draining stay in the same measured CPU bins; this does not claim that
rehashing is free. It allocates metadata and briefly holds the write lock.
Map hash seeds/IDs vary across processes. This comparison verifies 31,309,200
deliveries and 147,468 normal Close handshakes, with exact counters, zero body
compaction and complete final resource release in all six processes.

Eighteen separate four-second cost runs check ordinary 64-client traffic and
churn with the same frozen stress client. At 1 KiB, median throughput differs
by +0.81% and CPU/delivery by −0.40%, with p99 at 1.048576 ms in both builds.
Churn differs by +0.07% throughput and +0.12% CPU/lifecycle, with p99 at 2.097152 ms.
The short 64 KiB comparison had a higher median p99 bucket in the selected build,
so four ten-second alternating pairs followed. Those throughput medians are
205,511 versus 202,278 deliveries/s (−1.57%) and CPU is 11,530 versus 11,561
ns/delivery (+0.27%), with overlapping ranges and the same 4.194304 ms median
p99 bound. One selected long run has the 8.388608 ms bucket; all raw results
are retained. These guards support a scoped improvement after population waves,
not a general ordinary-traffic speed or latency improvement.

All 26 cost runs pass exact counts, error/resource checks and zero routed-value
compaction. Traffic deliberately drops 64 TCP handles at its deadline; every
departure is accounted for. Churn and the population probe separately verify
normal Close. The quiescent local runtime was updated after validation; a live
1,024-peer wave preserves typed global delivery, and all 1,026 peers close
cleanly. Fresh browser/legacy checks and default decoder reclamation also pass.

The [final comparison](benchmarks/2026-10-16-registry-peak-final-comparison.json),
[ordinary cost guard](benchmarks/2026-10-16-registry-cost.json),
[long large-frame confirmation](benchmarks/2026-10-16-registry-large-confirm.json),
[summary](benchmarks/2026-10-16-registry-summary.json), and
[manifest](benchmarks/2026-10-16-registry-build.json) bind traces and sources.
Reproduce with `cargo build --release --bins --example registry_peak`, then
`python3 scripts/registry-peak.py --waves 3 --peak-clients 8192 --seconds 3
--output artifacts/registry-peak.json`. The benchmark requires its harness gates.
Channel/membership-table capacity, mass departure within one room, much larger
active fanout, multi-hour behavior and deployed TLS/network paths remain outside
this optimization. Event-boundary RSS does not measure continuous peak RSS,
and freeing ID buckets does not imply that the allocator returns pages to the OS.

## Membership index capacity after room departures, 2026-10-16

The ID-index change did not cover ordinary room membership or the global
membership index scanned during private delivery. An initial 4,096-peer room
case showed little CPU change; its record is retained. At 8,192 transient peers,
an unprofiled ordinary-room run rose from 1,457 to 1,688 native ns/delivery after
departure. Private global-index delivery rose from 1,476 to 1,714 ns/delivery.
A separate profile put `Relay::receive` at 26.24% of user cycles, dominated by
its empty-bucket group scan. These observations preceded runtime changes.

One typed helper now trims sparse `u64 -> Arc<Peer>` ownership and membership
indexes. The prior thresholds remain: capacity above 256, occupancy at most one
eighth, and room for twice the remaining entries. Empty ordinary rooms retire
directly; the persistent global room keeps its variables. Only keys and `Arc`
handles move. Peer, queue, variable and published-payload storage stays intact,
with no additional per-message check, allocation or body movement. Rehashing
allocates metadata under the existing write lock; regrowth can allocate again.
The [stability regression](stability.md#membership-lifetime-during-room-departures-2026-10-16)
first fails on retained capacity, then checks pointers, held reservations,
administrative events, later delivery and both variable lifetimes.

The final comparison uses three fresh processes per build/scope, reversing order
on the second repetition. Four-worker relays use CPUs 0–3 and the same frozen
client uses 4–7. The two original peers stay connected through 8,192 transient
members in the same room. `room` measures ordinary broadcast; `private` measures
private delivery with both survivors in the global room and zero listener flags,
isolating its index scan from extra listener fanout. Three-second traffic phases
use 64 prebuilt 1 KiB values and 16-message windows. Every full encoded value,
sender, frame and FIFO position is checked. Stdin gates exclude filling/draining
from routing CPU. Rates/heartbeats are disabled, with storage/queue defaults
retained. CPU includes boundary management sampling at 10 ms tick resolution;
percentiles are fixed-histogram upper bounds. Hash seeds/key positions vary.

Every remaining peer verifies each disconnect ID and its exact FIFO prefix.
Sixteen-close windows wait for all readers, preserving default 64-message queues
and all notifications. Each process checks 33,566,720 departure deliveries and
normal Close/EOF. This is paced departure, not unpaced simultaneous closure.

| Post-departure median | Prior `3963138` | Selected |
| --- | ---: | ---: |
| Ordinary room deliveries/s | 403,031 | 440,729 |
| Ordinary room server ns/delivery | 1,696 | 1,488 |
| Ordinary room p99 upper bucket | 0.065536 ms | 0.032768 ms |
| Global-index private deliveries/s | 409,571 | 434,403 |
| Global-index private server ns/delivery | 1,636 | 1,494 |
| Global-index private p99 upper bucket | 0.065536 ms | 0.032768 ms |

Post-wave room CPU improves **12.28%** with **9.35% higher throughput**; private
CPU improves **8.68%** with **6.06% higher throughput**. Fresh CPU differs by
−0.64%/+1.44% respectively; fresh private p99 also differs by one bucket.
Filling CPU medians remain 0.16–0.17 seconds. Draining uses about 89 native
CPU-seconds per wave, dominated by preserved quadratic notifications and kept
separate from steady routing. These data support the post-wave improvement,
without a free-rehash or general ordinary-traffic speed claim.

All twelve processes verify 30,873,216 application deliveries, 402,800,640 exact
disconnect deliveries and 98,328 normal Close handshakes, with exact counts/bytes,
zero routed-body compaction and complete resource release. The selected profile
puts `Relay::receive` at 9.78% of user cycles. Control/selected profiles have
2,209/2,220 samples and zero lost samples; profiled timings are excluded.
The prior global-ID probe also passes three further waves in separate rooms.

Eighteen four-second ordinary cost cases use 64 clients and a frozen stress
client. Throughput/CPU medians differ by −0.98%/+0.23% at 1 KiB, −0.02%/−0.71%
at 64 KiB and −0.79%/+1.07% for churn. Ranges overlap, with matching p99 buckets
at 1.048576, 4.194304 and 2.097152 ms. Counts, departures, zero compaction and
cleanup pass. Traffic deliberately drops 64 TCP handles per case; churn and the
population probe separately verify normal Close. Live ordinary/global-room
waves verify 1,051,648 disconnects, 130 typed messages during/after departure,
2,052 clean closes and zero additional body copies. Variables survive retirement,
and ordinary variables release normally. Fresh Chrome/legacy/reclamation pass.

The [comparison](benchmarks/2026-10-16-member-comparison.json),
[ordinary guard](benchmarks/2026-10-16-member-cost.json),
[live record](benchmarks/2026-10-16-member-live.json),
[summary](benchmarks/2026-10-16-member-summary.json), and
[manifest](benchmarks/2026-10-16-member-build.json) bind commands and sources.
Build `cargo build --release --bins --example registry_peak`, then run
`python3 scripts/registry-peak.py --scope room --peak-clients 8192 --seconds 3
--output artifacts/member-room.json`. Use `--scope private` for the global
membership path; `--scope global` retains the separate-room probe. The following
section measures channel-name table costs. Larger active application fanout,
unpaced departure, long duration, TLS/proxies and clusters remain unmeasured.
Event-boundary RSS is not continuous
peak RSS; metadata trimming does not guarantee OS page return or cap process RSS.

## Channel-name capacity after room waves, 2026-10-16

After ownership and membership reclamation, the native channel-name map still
retained spare buckets from retired ordinary rooms. Both authenticated CBOR
channel queries and management inspection iterate this map. The first release
socket probe at 8,192 temporary rooms records fresh/post-wave query CPU of
1,945/2,054 ns and throughput of 474,278/444,151 queries/s. Every reply and all
management snapshots pass; this exploratory run is excluded from final medians.

A separate user-cycle profile includes the channel-name empty-bucket loop in
`Relay::receive` (inlined control handling). This is consistent with the
[standard-library iteration contract](https://doc.rust-lang.org/std/collections/struct.HashMap.html#method.iter),
which visits empty buckets as well as occupied entries. The profile identifies
the scan rather than establishing its full CPU cost or a general bottleneck.
The control has 2,138 samples and the selected recording 2,135, both with zero
lost samples; their `Relay::receive` shares are 8.36%/6.92%. Profiled timings are
diagnostic and excluded from performance comparisons.

The existing sparse-map policy is now generic over the registry's key/value
types and also runs after removal of an empty ordinary room. Capacity must be
over 256 and at least eight times the remaining length before shrinking, with
twice the remaining length retained as growth headroom. Rehashing moves channel
descriptors and handles; name/variable bytes, peer objects, queues and published
bodies keep their existing storage. Normal application messages and inspections
add no trimming check. Rehashing allocates metadata under the existing write
lock; another room wave can grow it again. This is not an RSS cap or an OS
page-return guarantee.

The frozen `registry_peak` client adds `--scope channels`: two original clients
stay in `re_globl` while 8,192 temporary peers join distinct ordinary rooms.
After authentication it sends channel queries in 16-message windows, comparing
every complete response with independent literal CBOR. An authenticated HTTP
snapshot at each gate independently verifies every field and base64 sort order,
including the 8,193-entry peak and the final empty global room. These snapshots
sit outside traffic CPU intervals. Queues keep their default 64-message limit.
Every connection checks its exact normal Close acknowledgement and EOF.

Three fresh processes per build keep those clients through two population waves.
Order reverses for repetition two; four-worker servers use CPUs 0–3 and the same
frozen client uses 4–7. Each phase lasts four seconds. Take the median of each
process's two post-wave phases, then the median across the three processes:

| Measurement | `c2278cf` control | Selected |
| --- | ---: | ---: |
| Fresh query CPU, ns/reply | 1,958 | 1,975 |
| Fresh replies/s | 471,130 | 468,278 |
| Post-wave query CPU, ns/reply | 2,047 | 1,951 |
| Post-wave replies/s | 442,038 | 470,346 |
| Post-wave p99 upper bucket, ms | 0.065536 | 0.032768 |
| Filling CPU, seconds for two waves | 0.34 | 0.34 |
| Draining CPU, seconds for two waves | 0.19 | 0.19 |

Post-wave CPU improves **4.71%**, throughput **6.40%**. Per-process post-wave
CPU ranges are 1,983–2,123 ns for control and 1,950–1,955 ns selected; throughput
ranges are 426,209–460,162 and 467,858–471,317 replies/s. Fresh CPU is +0.88%,
with the same 0.032768 ms p99 bucket. Preparation CPU includes gated management
inspection and is separate from routing/query timing. The result supports
query recovery after room waves, without a general traffic or capacity claim.

All six processes verify **33,165,328 queries and 98,316 normal Close/EOF
handshakes**, exact incoming/delivered counts and bytes, zero routing reuse/copy
counters for control queries, and final zero resource/error gauges. Three more
separate-room waves pass the prior global-routing probe. Eighteen ordinary cost
cases use the same frozen stress client across 1 KiB, 64 KiB and connection churn.
CPU medians differ by +0.56%, +0.19% and +0.13%; throughput by +0.77%, +0.18% and
−2.90%, with overlapping ranges and the same p99 buckets (1.048576, 4.194304 and
2.097152 ms). Native CPU covers socket setup through final transport cleanup,
excluding process shutdown. Every native process separately exits zero without
a forced kill. Traffic deliberately drops 64 TCP handles per case; churn checks
normal Close. No general ordinary-traffic speedup is attributed to this change.

The stability runner formerly ignored service exit status. A calibration wrapper
completes actual churn, forwards termination, waits for a native exit zero and
then deliberately exits 7. The old runner falsely passes; the corrected runner
rejects it after waiting, with exit status and shutdown logs retained. A separate
diagnostic verifier initially rejected `perf`'s expected SIGINT return after an
intentional stop. That rejected recording is preserved and excluded from
timings; a fresh completed recording passes after accepting only exit zero or
the deliberately sent SIGINT. The timed runner snapshot remains frozen.

The [comparison](benchmarks/2026-10-16-channel-comparison.json),
[ordinary guard](benchmarks/2026-10-16-channel-cost.json),
[live record](benchmarks/2026-10-16-channel-live.json),
[summary](benchmarks/2026-10-16-channel-summary.json), and
[manifest](benchmarks/2026-10-16-channel-build.json) bind commands and sources.
Build `cargo build --locked --release --bins --example registry_peak`, then run
`python3 scripts/registry-peak.py --scope channels --peak-clients 8192 --waves 2
--seconds 4 --output artifacts/channel-peak.json`. The runner provisions disposable
loopback-only benchmark credentials. Final timing cases use the frozen timed
runner; the current runner differs only in diagnostic `perf` stop handling.
Management inspection is functionally verified, not separately benchmarked.
Large active inspection/lock contention, unpaced departure, multi-hour operation,
TLS/proxies and clustered routing remain unmeasured. RSS samples are event-boundary
observations, not continuous peaks or an allocation census.

## Concurrent channel inspection, 2026-10-16

The next measured workload keeps 8,192 single-member rooms connected, plus two
private pairs and eight reconnect workers in distinct fixed rooms. Two optional
inspectors each request the full channel list every 10 ms. The first control
measurements show HTTP inspection reducing healthy throughput from about 569k
to 395k deliveries/s and increasing the p99 upper bucket from 0.131072 to
1.048576 ms. This is an exploratory observation, separate from the final matrix.

Both inspection paths held the registry read lock while sorting and encoding;
HTTP also formatted UTF-8/base64 under it. Waiting membership writers can then
delay other registry readers. The selected implementation captures coherent
bounded names and counts, releases the lock, then formats/sorts/encodes the
response. Inline names avoid per-name reference counts. CBOR captures only the
member counts it returns; HTTP also captures variable usage. It copies small
inspection metadata without changing application-body ownership or routing.

An initial shared-`Bytes` snapshot improved HTTP results but added CBOR CPU
cost. Separate user-cycle diagnostics found sorting and shared-name cloning;
that motivated the narrower inline representation. The initial prototype,
exploration and profiles remain recorded, and are excluded from the final
timing medians. Profiles identify sampled work, not a complete attribution of
lock-wait time.

Final medians use three fresh processes per build/mode, eight measured seconds
each, with mode order rotated and build order reversed on repetition two.
The four-worker server uses CPUs 0–3; the same frozen four-worker client uses
4–7. Setup, exact quiescent snapshots and cleanup are gated outside timing.
Each inspector verifies all names, counts and ordering; HTTP also verifies all
metadata fields. Binary, NUL and Unicode names are included. Every private
message checks its full 1 KiB value, sender and FIFO order in 16-message windows.
Every reconnect checks welcome, counter self-delivery, exact normal Close and
EOF. Queues retain their defaults; inbound rates and heartbeats are disabled.

| Concurrent inspection | Healthy deliveries/s, control → selected | Reconnects/s, control → selected | Healthy p99 upper bucket, ms | Reconnect p99 upper bucket, ms |
| --- | ---: | ---: | ---: | ---: |
| None | 535,030 → 533,004 | 53,597 → 52,853 | 0.131072 → 0.131072 | 1.048576 → 1.048576 |
| Authenticated CBOR, 200 queries/s | 495,409 → 529,973 | 49,617 → 52,764 | 0.262144 → 0.131072 | 1.048576 → 1.048576 |
| Authenticated HTTP, 200 queries/s | 401,109 → 472,039 | 35,259 → 39,016 | 1.048576 → 0.262144 | 4.194304 → 2.097152 |

CBOR inspection allows **6.98% more healthy deliveries and 6.34% more
reconnects**; HTTP allows **17.68% and 10.66% more** respectively. With no timed
inspection, throughput differs by −0.38%/−1.39%, with overlapping ranges and
unchanged p99 buckets. These are improvements under this concurrent inspection
workload, not general forwarding gains. All inspection cases complete exactly
1,600 timed queries per process. CBOR inspection p50 changes from 2.097152 to
1.048576 ms; its p99 remains 2.097152 ms. HTTP inspection p50/p99 stay in the
8.388608 ms bucket. Healthy HTTP-case p50 worsens from 0.032768 to 0.065536 ms,
despite the p99 improvement. Latencies include client validation and competing
client tasks; the percentile figures are logarithmic bucket upper bounds.

Server CPU for all concurrent work changes from 16.11 to 15.74 seconds without
inspection, 15.69 to 16.73 with CBOR, and 15.12 to 16.65 with HTTP. The latter
increases accompany more completed application work and cannot be interpreted
as isolated CPU per message or per inspection. Snapshot allocation and sorting
still scale with the full list; the change adds no cache or pagination.

All eighteen processes verify **71,136,608 healthy deliveries, 6,832,194
reconnect lifecycles, 19,200 timed inspections and 6,979,734 normal closes**.
Exact input/output counts and bytes pass, routed application-body compaction
stays zero, all final resource/error gauges clear, and every client/server exits
zero without forced killing. RSS is sampled at gates rather than continuously.

The [final comparison](benchmarks/2026-10-16-inspection-final-comparison.json)
contains medians, ranges, hashes and links to every raw trace. The
[initial shared-name prototype](benchmarks/2026-10-16-inspection-comparison.json)
is a separate candidate. The frozen timed client predates the later C++ protocol
adapter; its original source and runner are preserved under
`artifacts/clients/inspection`. Build `cargo build --locked --release --bins
--example inspection`, then run `python3 scripts/inspection.py --mode http
--output artifacts/inspection.json`; use `--mode cbor` or `--mode none` for the
companion workloads. These finite plaintext loopback results do not establish
TLS/proxy behavior, multi-hour stability or cluster capacity.

An [ordinary traffic/churn guard](benchmarks/2026-10-16-inspection-cost.json)
runs eighteen more fresh four-second cases with 64 clients. At 1 KiB, throughput
is +0.81% and CPU per delivery +0.08%; churn throughput is −3.74% and CPU
decreases 2.34%. Both retain their p99 upper buckets. The first 64 KiB
comparison records throughput −4.18%, CPU +2.47% and a worse p99 upper bucket
(4.194304→8.388608 ms). A separately recorded
[six-case follow-up](benchmarks/2026-10-16-inspection-large-cost-followup.json)
uses the identical 64 KiB workload and alternating order: throughput is +3.99%,
CPU −2.88%, with both p99 buckets at 8.388608 ms. Ranges overlap in both sets;
the initial adverse observations remain recorded. These short guards do not
establish a consistent ordinary large-message gain or loss. They support the
inspection-specific result without a general forwarding-speed claim. All
twenty-four guards verify every application value, exact joins/departures, zero routed-body
compaction, cleanup and process exit. Traffic deliberately drops 64 TCP handles
per case; churn separately verifies normal Close. Guard CPU includes setup and
final transport cleanup, excluding service shutdown.

## Direct C++ comparison, 2026-10-16

The inspection optimization above compares two Rust builds. A separate direct
comparison now runs the upstream shipped C++ executable (`app`, SHA-256
`ce95e4dea01a4db0e5c32942d5520b5f4a8ab1c87affdcd8120bc8aefa332547`)
from upstream revision `881c8daaf2cf05adea8f522b28cf372e8f82d4e9` against the
selected native Rust release. C++ uses the original binary protocol; Rust uses
CBOR. Both receive identical encoded byte-string values (1,024 application
bytes, 1,027 value bytes). C++ treats them as opaque; Rust validates CBOR. This
compares complete implementations and protocols, not language overhead alone.

One frozen four-worker load client contains small adapters for join, welcome,
authentication, routing, queries and departures. Private traffic uses the same
two pairs, 64 cyclic templates and 16-message windows as the inspection probe.
Both servers hold 8,192 other single-member rooms. Full values/senders/order,
every normal Close/EOF, authenticated query names/counts, and variable setup are
checked. Each process is fresh in a private network namespace; dependencies and
executable checksums are pinned. C++ build flags are unknown; it uses the shipped
executable with isolated OpenSSL 1.1/TBB dependencies rather than a fresh tuned
source build. Original source/history are retained.

Three eight-second repetitions per implementation/case rotate case order and
reverse implementation order on repetition two. Both servers are pinned to CPU
0 for the first matrix; Rust uses one worker. The second allows CPUs 0–3 for
both and gives Rust four workers. The unchanged C++ process settles at three
total threads in this environment; Rust has two/five respectively. A separate
line-buffered startup diagnostic records 31 failed listener binds and one
listening socket. Consequently the four-core allowance does not turn the
shipped C++ executable into a four-loop baseline. We record
observed CPU use rather than assuming the C++ source's hardware-concurrency loop
results in equivalent active parallelism. The client uses separate cores 4–7.
Setup/cleanup are gated outside timing. No builds, profiles or other load tests
overlap these measurements; rates/heartbeats are off and Rust queues are default.

| One-core workload | C++ healthy deliveries/s | Rust healthy deliveries/s | C++ reconnects/s | Rust reconnects/s |
| --- | ---: | ---: | ---: | ---: |
| Private only | 742,524 | 583,047 | — | — |
| Private + eight reconnect workers | 257,120 | 396,667 | 9,224 | 15,779 |
| Private + 200 channel queries/s | 674,045 | 511,033 | — | — |

C++ private throughput is higher: Rust is 21.48% lower in the private-only case
and 24.18% lower with queries. Private-only native CPU is 902 versus 1,228
ns/verified delivery (median of per-process ratios). Healthy private p99 is in
the same 0.065536 ms upper bucket. With churn, Rust completes 54.27% more healthy
deliveries and 71.07% more lifecycles; healthy p99 is 0.131072 versus 1.048576 ms.
Both reconnect p99s are 2.097152 ms. Both query cases complete all requested
inspections with 4.194304 ms query p99.

| Four-core allowance | C++ healthy deliveries/s | Rust healthy deliveries/s | C++ reconnects/s | Rust reconnects/s |
| --- | ---: | ---: | ---: | ---: |
| Private only | 734,305 | 602,004 | — | — |
| Private + eight reconnect workers | 255,090 | 550,759 | 9,185 | 57,783 |
| Private + 200 channel queries/s | 670,793 | 626,215 | — | — |

Rust is 18.02% lower for private-only throughput and 6.65% lower with queries.
Private-only per-process CPU medians are 914/1,443 ns per verified delivery for
C++/Rust. In the churn case Rust completes 2.16× the healthy traffic and 6.29×
the reconnects, using 2.132 server CPU-seconds per wall-second versus 0.850.
Healthy churn p99 is 0.131072 versus 1.048576 ms; reconnect p99 is 1.048576 versus
2.097152 ms. Query p99 is 4.194304 ms for Rust and 2.097152 ms for C++; both
complete exactly 1,600 timed queries per process. These are separate concurrent
workloads; the differences across rows do not establish a causal effect of
enabling queries on private throughput. Client CPU remains below its four-core
allowance. Raw records contain ranges and both processes' CPU use.

At the ready gate with roughly 8,200 connected sockets, median RSS is about
**19.7 MiB for C++ and 101 MiB for Rust** in both matrices. This is roughly a
fivefold Rust memory cost at this population, despite shared application bodies.
During churn C++ grows to about 33.4 MiB; Rust stays near 101–102 MiB. These are
event-boundary RSS samples, not continuous peaks or an allocation census, and
include each server's dependencies and operational features. The results identify
private-forwarding CPU and idle connection memory as concrete follow-up targets;
they do not support a general Rust-over-C++ performance claim.

C++ delays retired members/variables until its 30-second GC; the short churn
case verifies the exact retained member count in each fixed churn room before
that first collection. Rust removes those members immediately and verifies
zero final reservations/resources. Queries run without churn so both channel
lists have the same membership; C++ ordering is normalized only on the client,
while Rust's documented order is checked. The full mixed HTTP management case
has no original C++ counterpart. GC pauses, long-duration behavior, TLS, larger
fanout and a rebuilt/tuned multicore C++ executable remain outside this comparison.

All **36 cases** pass, checking **158,383,904 healthy deliveries, 2,197,118
reconnect lifecycles, 19,200 timed inspections and 2,492,234 normal closes**.
Rust exact metrics/bytes and zero body-compaction/resource/error gauges pass.
All clients and Rust servers exit zero without forced killing; C++ stops with
its expected SIGTERM status because it has no graceful process signal handler.
The [one-core matrix](benchmarks/2026-10-16-cpp-one-core.json) and
[four-core matrix](benchmarks/2026-10-16-cpp-four-core.json) link every raw trace,
binary/source hash, loaded-library checksum, observed thread count and command.
Reproduce using the commands in the [README](../README.md#verify-and-measure).

### Rebuilt four-worker C++ source

The shipped artifact's failed listener binds make it unsuitable for judging
multicore C++ scaling. A second baseline compiles the preserved `relay.cpp` and
uWS sources with the upstream build script's `-O3 -march=native -mrdrnd
-fomit-frame-pointer -std=c++1z` flags, using Clang 23.1.1 and current system
OpenSSL/TBB/libuv/zlib. A temporary source copy changes only the worker-count
expression to the constant four, matching Rust's four workers and CPU allowance.
Production/upstream files are unchanged. Every fresh C++ process is checked for
four listening sockets and six total threads before the client starts. Rust
uses its normal Cargo release build; these compiler/library configurations are
recorded, not treated as an isolated language comparison.

Three fresh eight-second repetitions per build/case use the same frozen client
and application work, rotating private/query order and reversing build order in
repetition two. Both use CPUs 0–3, the client 4–7. Final medians and ranges:

| Measurement | Rebuilt C++, four workers | Rust CBOR, four workers |
| --- | ---: | ---: |
| Private deliveries/s | 1,110,733 | 615,940 |
| Private throughput range | 1,106,959–1,114,843 | 599,497–619,511 |
| Private p99 upper bucket, ms | 0.065536 | 0.131072 |
| Private server CPU, ns/verified delivery | 1,074 | 1,449 |
| Private ready RSS, MiB | 20.49 | 101.10 |
| Healthy deliveries/s with 200 channel queries/s | 686,055 | 630,576 |
| Query-case healthy throughput range | 681,759–1,085,770 | 627,520–636,578 |
| Query p99 upper bucket, ms | 4.194304 | 2.097152 |

C++ private throughput is **1.80× Rust's**; Rust is 44.55% lower. Private server
CPU per delivery is 34.93% higher for Rust, while its ready RSS is about 4.93×.
The query case has substantial C++ run-to-run variation, so the 686k median must
be read with its 682k–1,086k range. Rust is 8.09% below that median and has a lower
query p99 bucket in this matrix. Work placement among C++ reuse-port listeners
was not controlled or measured per thread; the data do not isolate the reason
for the variation. Both complete every requested query, and latencies include
client-side response validation. These are separate concurrent workloads, not
a test of adding queries to an otherwise fixed execution schedule.

The rebuilt multicore **churn case fails verification** and has no valid
throughput result. Three separate short checks record a timeout, a self-delivery
mismatch, and—with diagnostic welcome-ID tracking enabled—a repeated ID
`30936709452072125` during `churn0`'s join at sequence 550. Each had four verified
listeners and finished before the first C++ GC, while retired IDs should still
be reserved. This establishes the observed ID/lifecycle failure; it does not
isolate which source-level race produced it. The single-active-loop shipped
executable's successful churn results above cannot substitute for a successful
four-worker source run. Rust's separate four-worker churn cases pass every
delivery/lifecycle and all final resource checks.

All **twelve successful rebuilt/Rust cases** check **76,130,992 healthy
deliveries, 9,600 timed inspections and 98,376 normal closes**. Exact native
Rust counters/bytes, zero body compaction and final resource release pass; all
clients/Rust servers exit zero, and C++ receives its expected SIGTERM without
forced killing. The [rebuilt comparison](benchmarks/2026-10-16-cpp-rebuilt.json)
contains source/configuration/compiler/library hashes and every raw trace. The
[separate diagnostic record](benchmarks/2026-10-16-cpp-rebuilt-churn-diagnostics.json)
preserves all three failed checks and the shipped artifact's startup failures.
Diagnostic runs are excluded from successful timing medians.

Use `python3 scripts/compare-cpp.py --rebuild-cpp --workers 4 --cases private
query --output artifacts/cpp-rebuilt.json` after building the Rust binaries and
inspection example. The runner builds C++ once before timing. `--audit-ids` is
a fault-diagnosis option with extra client storage/work and must not be mixed
into ordinary performance results. Compiler tuning parity, per-thread work
placement, long GC cycles, TLS and sustained large populations remain open.
