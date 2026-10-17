# Continuing goal status

The user's ongoing goal is to port the upstream relay faithfully to Rust and keep
improving simplicity, code, documentation, stability, performance, concurrency,
and adjacent platform management. A later instruction makes CBOR the primary
protocol and preserves the upstream's zero-copy routing intent. The latest
instruction makes 100,000 simultaneous users an explicit operating target.
This file records reviewable checkpoints; they do not complete the open-ended
improvement goal.

## Latest verified checkpoint: 100,000 users, 2026-10-16

The [capacity evidence](performance.md#100000-connected-users-2026-10-16)
now covers real 100,000-user populations, with every welcome ID and full routed
body checked. Two complete waves on the previous selected runtime verify
13 million stable-user messages, 133,120 additional user join/Close/EOF cycles,
and final resource retirement. A new optional kernel-buffer policy closes an
observed memory-management gap: application queue limits alone do not constrain
the TCP payload buffered before backpressure. Native settings request send and
receive buffers once per accepted socket, preserving OS policy when unset.
The new runtime passes 141 Rust tests, strict Clippy, formatting and a real
accepted-socket `getsockopt` check. The data path keeps its shared CBOR bodies,
vectored writes, exact global reservations and existing ownership/locking model.

The new runtime also passes 100,000 stable users sending one 1 KiB message per
second for 65 seconds with heartbeats, 66,560 additional churn sessions and
eight deliberately unread recipients. With the final 16 KiB send / 64 KiB
receive profile, stable-message p99 is the 0.524288 ms histogram bucket and
measured RSS is 112.45 MiB. An initial matched mostly idle
comparison keeps a 1.048576 ms p99 bucket and cuts unread-peer buffered full frames from
a median 2,427.5 to 28, with a 1.60% lower sidecar healthy-throughput observation.
That is one matched pair, not a universal throughput estimate or allocated-memory
measurement. A near-64 KiB packet check finds hundreds of milliseconds of
loopback latency with the initial 16 KiB receive request; a 64 KiB receive
request reduces its observed maximum to 1.146 ms. The final profile retains
the small send buffer and uses that larger receive window.
The [operating profile](operations.md#100000-native-connections)
includes 10% TCP admission headroom, descriptor/memory settings and bounded
kernel/application storage. The CI case exercises a smaller real population.
Final acceptance routes fresh messages through all 100,000 users after a
65-second mostly idle/heartbeat/pressure interval, then verifies every normal
Close/EOF and descriptor/resource recovery. The selected native hash begins
`5c22e737`; the local CBOR and legacy services now run the frozen selected
binaries. Real native traffic and an independent legacy binary-message/Close/EOF
probe pass on those running services, with zero remaining sessions.

A worker-local weak route-cache prototype is removed after measurement: its
small dense-flow benefit does not carry to the 100,000-user overload workload,
where median CPU per message rises 1.57%. Offering one million messages/s to
four cores creates substantial backlog (about 518,162 verified/s and 268.435456 ms
p99); eight cores improve the observed rate but still miss that offered load.
A separate diagnostic attributes 75.35% of server CPU time to the kernel.
Larger single rooms, mass unread populations, long WAN/TLS runs and multi-hour
operation remain outside this finite capacity evidence.

## Prior verified checkpoint: 2026-10-16

The [forwarding study and larger active-pair comparison](performance.md#forwarding-counter-experiments-and-64-active-pairs-2026-10-16)
adds a verified 64-pair client with configurable runtime threads and a 16-pair
CI case (`cd25cdb`). At 64 active pairs plus 8,192 idle rooms, three fresh
repetitions measure Rust 2,062,414 / rebuilt C++ 2,213,321 private deliveries/s:
Rust −6.82% throughput, +16.62% CPU per delivery and −17.78% measured RSS, with
the same p99 bucket. All twelve private/query cases verify 141,970,336 healthy
deliveries, 7,200 queries and 99,864 normal Close/EOF exchanges. Query ranges
overlap and neither process exhausts its CPU allowance; this is not a saturated
capacity claim. Separate two/64-pair profiles and 48 ordinary counter-layout/
striped-counter comparisons do not justify either prototype. Both are removed;
the selected native executable remains byte-identical. The current client and
restored runtime pass 138 Rust tests, strict Clippy, formatting, the local CI
commands and an older frozen-client replay.

The refreshed [direct C++ comparison](performance.md#current-direct-c-comparison-2026-10-16)
measures this exact selected runtime in 24 fresh passing trials. Four-worker
private throughput medians are C++ 1,094,299 / Rust 1,021,578 verified deliveries/s
(Rust −6.65%, overlapping ranges); Rust uses 11.08% more CPU per delivery and
17.91% less measured RSS, with the same p99 bucket. Against the shipped
single-listener C++ executable on one core, Rust private throughput is +3.65%;
beside eight reconnect workers it delivers +54.98% healthy traffic and 2.78×
complete reconnect cycles. All bodies, ordering, Close/EOF and native counters
and resource cleanup pass. These are short loopback measurements with two
active pairs; the earlier fivefold RSS penalty belongs to the retired port.

Runtime `ceda58f` stores resumable HTTP upgrade metadata inline with the live
decoder. Its 288-byte state fits the existing 320-byte stage and 512-byte
connection on the measured Linux build. Removing the temporary box avoids an
allocation at acceptance and at each `WouldBlock` resumption; input/frame
buffers retain their ownership. The fixed handshake deadline, quotas and shared
CBOR routing preserve their behavior. A fragmented HTTP upgrade now completes
after slab compaction and verifies a full CBOR body. Embedding cancellation also
checks a pending HTTP descriptor and its read reservation beside 64 joined peers.

Four separate allocator diagnostics keep the healthy client quiescent while
256 incomplete HTTP peers receive 32 paced bytes each. The old drive stack
allocates exactly 8,192 objects/2,359,296 bytes; the selected stack allocates zero.
Whole drip-phase requests fall 9,843 → 1,652 objects (−83.2%) and 4,417,424 →
2,059,152 bytes (−53.4%), including fixed HTTP metric/dump overhead. Zero-peer
controls match exactly. Pending live storage falls 256 objects/73,728 bytes
without growing the connection slab. These are requested allocations under a
replacement allocator, not RSS or a general zero-allocation claim.

All 66 ordinary comparison processes pass: 48 pending-header cases with
zero/1,024/4,096 peers and 18 private/reconnect guards. Throughput ranges overlap
in every cohort. Pending-case medians range −2.39% to +1.26%; private/mixed/
reconnect guards also overlap. Two of eight pending-case median p99 buckets
move in opposite directions within the observed bucket ranges. Allocation
reduction and simpler ownership are retained, without a general speedup claim.
The [performance record](performance.md#inline-http-upgrade-state-2026-10-16)
records full rates, ranges, costs and fixture limits.

Verification passes 138 Rust tests, strict Clippy/formatting, release builds,
the new unpinned pending-header CI command, 48 common C++ wire cases plus eight
documented differences, seven JavaScript checks, four management checks and
seven Chrome checks. Three descriptor-pressure cases, four setup-fault cases,
two active-flood shutdown cases and separate 8,192-idle/1,024-refusal checks pass.
The 1,024-reader changing-room case verifies 445 membership cycles and 910,336
fanout deliveries. Four socket cases verify 9,049,584 ordinary deliveries,
561,273 reconnects across 4,096 source IPs, 212,643 healthy deliveries beside an
unread peer and 787,536 mixed-size fanout deliveries, with final cleanup and
normal process exits. Native/bridge services on 1338/1337 run the selected frozen
binaries, management is on 1339 and browser acceptance on 4180. Chrome is clear,
with four native listeners and zero retained resources. The
[manifest](benchmarks/2026-10-16-handshake-inline-build.json) binds source,
binaries, clients and evidence. Acceptance is local; the goal remains active.

## Previous checkpoint: accept-turn fairness, 2026-10-16

Runtime `d47f0fd` reduces native accept turns from 64 sockets to 16. This single
constant changes scheduling balance during bursts; the 1,024 backlog, bounded
continuation token, resource-error recovery and shared CBOR routing retain their
behavior. Helper `4e7deb8` adds gated perf diagnostics and user/system CPU
accounting to the admission workload. Separate profiles record 4,605/4,735
samples with zero lost samples; their timing is excluded from ordinary medians.

A 32-case pilot compares 8/16/32/64 before retaining 16. The main 42-process,
three-round comparison gives one-worker established echo gains of 39.64% under
global admission and 50.05% under direct-IP admission, with p99 halved from
1.048576 to 0.524288 ms. Refusal throughput falls 3.12%/3.98% and server CPU per
wall second rises about 3–4% with the changed work mix. At 64 refused clients,
echoes rise 12.45%, p99 halves and refusals fall 5.48%. All runs have zero kernel
listen drops; SYN retransmissions total zero before and three after. Ordinary
controls and four-worker echo ranges overlap. No general CPU-saving or
production-capacity claim follows.

Eighteen additional routing guards pass. Standard four-worker private, mixed
private and reconnect medians change −0.19%/−0.34%/−0.22%, with overlapping
ranges and unchanged p99. With 64 admitted reconnect workers on one native
worker, private throughput rises 7.49% and its p99 halves, while full reconnect
throughput falls 0.85% with overlapping ranges. The
[performance record](performance.md#accept-turn-fairness-2026-10-16) retains
rates, ranges, costs and workload limits. All 60 retained timing processes run
sequentially, separately from builds, profiling, stress and injected faults.

Verification passes 138 Rust tests, strict Clippy/formatting, locked release
builds, the small unpinned CI command, 48 common C++ wire cases plus eight
documented differences, seven JavaScript checks, four management checks and
seven real Chrome checks. Three descriptor-pressure cases, four socket-setup
fault cases and four shutdown-during-flood cases pass. Both builds complete
the separate 1,024-client refusal check with no kernel drops/retransmissions;
selected verifies 276,624 echoes and 494,166 refusals. The selected 1,024-reader
changing-room check verifies 347 membership cycles and 709,632 fanout deliveries.
Separate stress verifies 8,744,080 ordinary deliveries, 548,418 full reconnects
across 4,096 source IPs, 205,260 healthy deliveries beside an unread peer and
719,280 mixed-size fanout deliveries, with final cleanup and normal server exit.

Native/bridge services on 1338/1337 run the selected frozen binaries, with
management on 1339 and browser acceptance on 4180. Chrome has a clear console,
four native listeners and zero retained resources. The
[manifest](benchmarks/2026-10-16-accept-fairness-build.json) binds source,
binaries, clients and evidence. All acceptance is local; the goal remains active.

## Previous checkpoint: bounded admission and backlog, 2026-10-16

Runtime `8676e7e` bounds each native worker to one queued accept continuation
and applies a configurable TCP backlog to every data listener. Previously,
repeated listener events could enqueue duplicate admission turns; the initial
listener also had backlog 128 while additional workers used 1,024. Both public
CLIs now accept `--listen-backlog` / `WAR_LISTEN_BACKLOG`, default 1,024 and
subject to the OS cap. Native embedding applies `Config::listen_backlog` to
the supplied listener too; bridge embedding preserves its supplied listener.
Shared CBOR body ownership and application routing retain their behavior.

The initial actual-TCP regression fixture fails on the baseline with two queued
continuations instead of one. A separate isolated 1,024-client refusal diagnostic
finds 2,138 kernel listen drops, 1,416 SYN retransmissions and a client deadline,
while the admitted CBOR peer keeps making progress and relay accept errors stay
zero. With the selected backlog, that workload completes 530,535 refusals and
157,312 verified echoes, with no kernel drops or retransmissions.

The ordinary comparison passes 48 sequential fresh processes: three alternating
rounds, one/four workers, global/direct-IP admission, zero/64/256 refused TCP
clients, plus a coalescing-only intermediate. Independent echo/flood generators
have separate CPU sets and each case has a fresh network namespace. Across the
21 matched runs per variant, kernel listen drops fall from 6,655 to zero and SYN
retransmissions from 6,655 to one. The largest observed refused-TCP lifecycle
falls from 3,076 ms to 50.3 ms in the one-worker/global 256-client cohort.

This is a reliability and bounded-scheduling change. Saturated one-worker
refusal throughput rises 7.8–9.3%, while established echo throughput falls
28.0–29.8%; the baseline's kernel drops had throttled incoming work. Four-worker
flood cases show 3.6–11.7% more completed refusals and 1.6–2.6% fewer echoes.
Ordinary echo medians are +1.81%/−0.70% for one/four workers. Healthy median p99
buckets stay unchanged, while refused-TCP p99 can rise. The
[performance record](performance.md#tcp-admission-continuations-and-backlogs-2026-10-16)
retains rates, ranges, tails and the larger kernel-queue cost; no general speedup
or production capacity claim follows.

Verification passes 138 Rust tests, strict Clippy/formatting, locked release
builds, the small unpinned CI command, 12 live backlog-setting cases, six shutdown
diagnostics, three descriptor-pressure cases and four socket-setup fault cases.
The preserved C++ comparison passes 48 common cases and eight explicit
differences. Seven JavaScript, four management API and seven real Chrome checks
pass. Separate socket cases verify 6,793,728 ordinary deliveries, 519,848 full
reconnects across 4,096 source IPs, 204,361 healthy deliveries beside an unread
peer and 742,176 mixed-size fanout deliveries. Native/bridge services on 1338/1337
run the selected frozen binaries; management is on 1339 and browser acceptance
on 4180. Chrome has a clear console, four native listeners and zero retained
resources. The [manifest](benchmarks/2026-10-16-accept-backlog-build.json) binds
source, binaries, clients and evidence. Acceptance is local; the goal remains active.

## Previous checkpoint: recipient storage reuse, 2026-10-16

Runtime `0f70200` reuses uncaptured broadcast recipient storage across joins
and departures. The registry write lock plus exclusive `Arc` ownership permit
in-place handle updates. An active capture keeps its immutable view and the
cache follows the previous invalidation path. Departed peers retire immediately;
retained rooms shrink sparse handle capacity. Shared CBOR body ownership,
sender ordering, global broadcasts and observer routing retain their behavior.

Helper `b01b166` adds complete membership churn inside a busy room: every
temporary reader verifies its window, exact Close/EOF and departure at every
survivor. A corrupted departure at a secondary reader is rejected. Linux CI
commit `e52a953` runs a small unpinned two-worker case; that exact command
passes locally. The selected source passes 134 Rust tests, strict Clippy,
formatting and locked release builds.
Harness `d65308d` also resolves source snapshots beside older frozen clients;
a separate check verifies attribution against their preserved files.

Four separate allocator diagnostics measure requested allocations per complete
membership cycle falling from 32.82 to 30.83 objects and 15.24 to 11.10 KB
(−27.2% bytes). Stable-room totals match exactly, and both builds return to
identical live allocations after cleanup. Reuse keeps a small metadata cache
alive between updates; growth can retain spare handles, with sparse retirement
covered by a 1,024-member wave test. No application values enter that cache.

Forty-eight ordinary comparison processes and twelve private/reconnect guards
all verify exact bodies, matrix/counters and final cleanup. Fanout throughput
medians range from −0.96% to +1.31%, with seven of eight ranges overlapping and unchanged median p99
buckets. The pure-private guard is −1.74% with overlapping ranges; mixed private
is +0.12% and reconnects +2.00%. Allocation reduction is the retained benefit;
no general throughput or CPU-saving claim follows. Separate profiles retain
3,369/3,349 samples with zero lost samples. The
[performance record](performance.md#recipient-views-during-membership-churn-2026-10-16)
states the gated-workload, cold-room, allocator and timing limits.

Further checks pass the 1,024-reader changing-room workload, 48 common C++ cases
plus eight documented differences, three descriptor-pressure cases, four socket
setup fault cases, seven JavaScript checks, four API checks and seven Chrome
checks. Separate socket cases verify 8,807,056 ordinary deliveries, 551,622 full
reconnects across 4,096 source IPs, 206,893 healthy deliveries beside an unread
peer and 740,016 mixed-size fanout deliveries. Native/bridge services on 1338/1337
run the selected frozen binaries; management is on 1339 and the browser check
on 4180. Chrome has a clear console, four native listeners and zero retained
resources. The [manifest](benchmarks/2026-10-16-room-churn-reuse-build.json)
binds source, clients, binaries and evidence. Acceptance is local; the goal
remains active.

## Previous checkpoint: socket setup recovery, 2026-10-16

Runtime `9a941637c63f5bd34a82c27db985cdeba2388c31` keeps accepted native
socket setup failures from stopping the relay. A failure configuring TCP or
registering readiness retires that socket, increments `war_setup_errors_total`
and `war_rejected_total`, and suspends only that worker's listener for one
second. Existing socket work, timers and shutdown continue. Early setup refusals
now retain global/IP admission through physical TCP close, using explicit local
drop order. Routing and shared payload ownership are unchanged.

The independent Linux probe reproduces both failures in one/four-worker
baseline processes: all four exit 1, and TCP-configuration failure releases
its slot before TCP closes. Selected debug/release builds pass eight cases
with 24 injected failures, two queued recoveries and a shutdown wave per case.
Every held descriptor retains its slot; recovery restores admission/read
allowance and exact counters. The existing three descriptor-exhaustion cases
also pass. A shutdown-oracle correction preserves the existing behavior:
shutdown flushes `1001` then retires TCP; ordinary recovery clients still
require their exact Close acknowledgement and EOF.

Twelve ordinary before/after processes preserve performance. Pure-private
median is −0.41% with overlapping ranges; mixed private is +1.21% and reconnect
throughput +2.02%. All p99 buckets are unchanged. Mixed CPU rises with completed
work; these are stability guards, without a general speedup or CPU-saving claim.
The [performance record](performance.md#accepted-socket-setup-recovery-2026-10-16)
retains ranges, methods, costs and scope.

Current verification passes 131 Rust tests, strict Clippy/formatting, locked
release builds, seven JavaScript checks, four management API checks, seven real
Chrome checks, 48 common C++ cases plus eight documented differences, and the
1,024-reader fanout upper bound. Four separate socket cases verify 8,791,152
ordinary deliveries, 550,559 complete reconnects across 4,096 source IPs,
208,104 healthy deliveries beside an unread recipient, and 747,792 mixed-size
fanout deliveries. Resources retire and every disposable server exits normally.
Native/bridge services on 1338/1337 now run the selected frozen binaries, with
management on 1339 and the browser check on 4180. Chrome has a clear console,
zero retained resources and four native worker listeners. The
[manifest](benchmarks/2026-10-16-setup-pressure-build.json) binds source,
binaries, helpers and evidence. All acceptance is local. The goal remains active.

## Previous checkpoint: large-room fanout, 2026-10-16

Runtime `b410571e769d9b0b70fa6f5517fc6576d898ef24` improves large-room
broadcast fairness while preserving shared payloads. Ordinary channel broadcasts
without global channel observers capture a reusable peer view, release the
registry before output, and charge native read turns for recipient work.
Each message finishes against one coherent view before yielding; membership
changes invalidate the view. Global/observer routes retain their prior path.
No application values enter the metadata cache.

Helper commit `c8b55d394c92c0f2bdf4f7bf5a299239397dbc53` adds verified fanout
alongside private traffic and reconnects, plus independent client CPU sampling
and server worker selection. A corruption test proves validation reaches a
non-primary reader. Kernel socket ownership separates shared/separate source
workers in 30 final before/after processes; 23 setup-only placement rejects are
excluded before traffic. The initial quota-only candidate is rejected and
retained as evidence. All accepted runs verify every body/sender/FIFO, the full
reader/message matrix, exact departures, counters, normal Close/EOF and cleanup.

With 512 readers, reconnect p99 improves from 67.109 to 8.389 ms buckets.
Four-worker private throughput is 2,119 → 18,295 deliveries/s for shared sources,
and 182,615 → 418,389 for separate sources. Fanout throughput decreases about
3–6%. The no-broadcast private median is −1.50% with overlapping ranges;
a six-process pure-private guard is +0.41%, and a six-process two-peer fanout
guard is −0.32%, both with overlapping ranges and unchanged latency buckets.
CPU rises where more independent work completes; no general CPU saving or
production-capacity claim follows. The
[performance record](performance.md#large-room-fanout-fairness-2026-10-16)
retains full methods, ranges, costs and scope limits.

Separate allocator diagnostics measure 256 KiB extra channel metadata at 8,192
idle rooms, and 4,144 bytes/two allocations when the 513-member view is warmed.
An old 8 KiB routed body retains its original pointer and byte reservation
through view replacement, peer removal and final room destruction. Stable
membership reuses its cached view. Three profiles have zero lost samples;
all are separate from ordinary timing.

Current verification passes 131 Rust tests, strict Clippy/format checks, release
builds, 48 common C++ compatibility cases plus eight documented differences,
seven JavaScript checks, four API checks and seven real Chrome checks.
All three descriptor-pressure recovery cases still pass. Separate socket
workloads verify 9,148,800 ordinary deliveries, 540,686 complete churn cycles
across 4,096 source IPs, 208,477 healthy deliveries beside an unread recipient,
and 737,424 mixed-size fanout deliveries, with complete cleanup and normal
process exit. The probe’s 1,024-reader upper bound passes too. An admission-test
assumption about parallel global/IP refusals was corrected while keeping exact
total refusals, serial IP increments and global-first priority checks.

The active branch is `codex/lean-core`. Native sockets use fixed Mio readiness
workers, shared read storage and direct vectored writes; Tokio handles management,
signals and the optional legacy bridge. Existing terminal shutdown, descriptor
retry, optional IP admission and sparse metadata/queue retirement remain intact.
Local native/bridge services run the selected frozen binaries on 1338/1337,
management on 1339, and the browser check on 4180. Fresh Chrome acceptance passes
all seven checks, with a clear console, zero retained resources and four native
listeners. The [manifest](benchmarks/2026-10-16-fanout-view-build.json) binds the
source, frozen clients, selected/rejected binaries, raw checks and diagnostics.
These are local checks; no hosted deployment or GitHub CI execution is claimed.
The ongoing goal remains active.

## Earlier implementation checkpoints

- Upstream downloaded at `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`; source/history
  and MIT attribution retained on local branch `codex/rust-relay`.
- Rust/Tokio CBOR relay: channel and global broadcast, cross-channel private
  messages, listener flags, typed channel variables, entropy-backed IDs, fixed-ID
  takeover, and disconnect events.
- One CBOR routing envelope. Definite-length values are bounded and forwarded
  verbatim. Browser/Node-oriented JS client preserves uint64 IDs as BigInt and
  serializes control requests.
- Separate optional legacy bridge; original `relay.js` exercised unchanged, plus
  modern/legacy byte-string and text interoperability.
- Bounded handshake/join/admission, per-peer and process queue budgets, channel
  and process variable budgets, inbound rates, heartbeat, write and shutdown
  deadlines. RAII cleanup and takeover ownership checks prevent stale removal.
- Separate management listener with liveness/readiness, Prometheus, authenticated
  channel inspection, drain/resume, and administrative disconnection.
- Release socket workload, independent CBOR and legacy vectors, Rust network
  integration checks, client checks, CI workflow, protocol and operations docs.
- Earlier checkpoint checks: 86 Rust tests and 7 JavaScript socket cases pass, along with
  strict Clippy and formatting. CI is supplied but has not run on GitHub.
- Read buffer changed from 128 KiB to 4 KiB after repeated 2,000-socket measurements
  showed about 91% lower idle RSS. Throughput variability is documented separately.
- Shutdown suppresses quadratic disconnect traffic; output limits allow channel
  queries larger than incoming application-message limits.
- Sustained checks found and fixed missing Close acknowledgements and delayed
  eviction of unread recipients. Shared close handling covers native CBOR and
  legacy clients, before and after join. Pending writes observe policy cancellation;
  routing membership/queued reservations release before a graceful handshake.
- A 30-second rerun verified 35,328,176 deliveries. A 20-second churn rerun completed
  1,911,994 connection lifecycles with 64 workers. An unread recipient was removed
  by the first 250 ms post-overflow sample while all 530,408 healthy deliveries
  were verified. All cases returned to zero sessions/connections/queued bytes and
  the persistent global channel. See stability.md for method and caveats.
- Repeated, affinity-separated socket measurements reproduced the cancellation
  overhead. Reusing pinned terminal waits recovered it: median 1,623,686 verified
  deliveries/s versus 1,384,284 for the fixed-close control, with 17.1% less server
  CPU per delivery. Close/eviction behavior remains intact. Idle state cost rose
  about 0.50 MiB at 2,000 sockets. See performance.md for raw evidence and limits.
- Native forwarding now shares received application storage through fanout and
  vectored socket output. The small sender prefix is separate, so varying CBOR
  integer widths require no body movement. Tiny views after enlarged input are
  compacted before publication; ordinary small messages share the normal slab.
- Final repeated 64 KiB traffic used 27.4% less server CPU per verified delivery
  with 4.6% higher median throughput. At 1 KiB, throughput ranges overlapped and
  CPU per delivery rose 2.1%. The copy counter stayed zero in all traffic runs;
  pointer checks cover queue and write ownership. Median idle RSS rose 0.86 MiB
  at 2,000 sockets. Raw evidence and limits are in performance.md.
- Final lifecycle checks verified 917,429 cumulative reconnects and 259,154
  healthy deliveries alongside an unread client, with complete cleanup. Finite
  fanout checks verified 31,500 deliveries per run. Fragmented input, interleaved
  controls, partial writes, and all WebSocket output length boundaries are covered.
- Local runtime refreshed on 2026-10-15: native CBOR at `127.0.0.1:1338`, legacy
  bridge at `127.0.0.1:1337`, management at `127.0.0.1:1339`. Both management
  health/readiness checks returned 200. Three socket checks against these running
  release processes exercised the unchanged original client, mixed CBOR/legacy
  values, and clean Close. A separate release workload verified 16 private
  deliveries. Gauges returned to zero afterward. Binary/process evidence is in
  `benchmarks/2026-10-15-zero-copy-live.json`; this is a timestamped observation.
- Sustained mixed-size fanout now verifies every reader's frames, bytes, ordering,
  disconnect events, and simultaneous graceful departure, with bounded templates,
  histograms, and acknowledgements. Optional unread and delayed readers exercise
  concurrent data and control pressure.
- A 128-client departure wave reproduced resets when a capacity signal cancelled
  output before its first poll. The connection now distinguishes untouched output
  from a possibly partial frame and preserves an already queued peer Close on
  the safe closing path. Three old-build runs failed; all three fixed runs passed.
- Final-build 30-second / 128-client mixed-size acceptance verified 3,440,304
  deliveries to 126 healthy readers beside one unread peer and a delayed reader.
  Peak sampled RSS was 13.96 MiB. Routed bytes were 99.784% reused; the existing
  small-view compaction policy accounted for the rest. A repeated 1 KiB cost guard
  recorded 1,582,701 versus 1,536,783 median deliveries/s with the same p99 bucket;
  no general performance gain is attributed to these correctness changes. See
  stability.md for the separate implementation stages, outcomes, and limits.
- The current local release processes were refreshed after that fix. A real Node
  client group completed 128 simultaneous clean closes with code 1000, and three
  additional live checks exercised the original client, native/legacy values, and
  Close before/after joining. Readiness/liveness returned 200 and gauges returned
  to zero. The timestamped binary/process snapshot is in
  `benchmarks/2026-10-15-graceful-close-live.json`.
- A further regression reproduced missing Close acknowledgement after input
  quota exhaustion. Both receive loops now handle a valid Close before rate
  accounting; the regression exercises this before and after joining. Other
  inbound traffic retains its limits.
- A 17-byte incoming application limit also rejected valid 125-byte WebSocket
  control frames. The frame ceiling now permits RFC-sized controls while the
  independent application-message limit still rejects 18-byte complete and
  fragmented data. Both real-socket boundary checks pass in debug and release;
  full-sized Ping/Pong/Close work before and after joining.
- A pinned esbuild development dependency produces a standalone ES2022 browser
  module and a small acceptance page. Actual Chrome 155 execution imported that
  generated library, passed seven checks (typed/large delivery, exact uint64,
  concurrent variables, disconnect, clean Close), and recovered from an invalid
  URL. Desktop and 390-pixel-wide output were inspected. The browser check
  routed 131,173 shared value bytes with no body compaction and returned all
  session, connection, queue, and variable gauges to zero. Source/binary hashes
  and timestamped observations are in
  `benchmarks/2026-10-15-browser-control-acceptance.json`.
- The optional local bridge was restored after one unexplained clean process
  exit. Three live compatibility/Close checks passed through its replacement.
  The recorded exit code was zero, with no error output; no code defect or
  recurrence has been established. Retain this observation for a later lifecycle
  investigation if it recurs.
- A malformed-CBOR campaign found that expanded simple values `f8 00` through
  `f8 1f` passed the decoder and reached a recipient. A check confined to simple
  values now rejects these RFC-invalid forms without changing payload storage.
  The real-socket regression rejects 192 routed/nested/map-key/variable cases,
  including 80 fragmented values with interleaved Ping. It also verifies 248
  valid simple-value deliveries byte for byte and healthy recovery. Independent
  unit vectors cover truncation, reserved headers, malformed UTF-8, nonminimal
  definite headers, duplicate keys, floating-point bits, and exact depth/item
  boundaries. Debug and release network checks pass.
- Repeated receive-slab retention checks now alternate large unknown-target
  primers with tiny output held behind an unread recipient. All six fresh-server
  cases passed full delivery/ordering, Close, counter, and cleanup checks. Raising
  the primer count from 64 to 256 changed median sampled peak RSS from 15,828 to
  15,920 KiB; the large forwarded values reused 8,388,648 encoded bytes in each
  case, while only 211/981 tiny value bytes were compacted. The existing policy
  is unchanged. Method, raw records, and finite-workload limits are in
  `performance.md` and `benchmarks/2026-10-15-retention-pressure.json`.
- Current local native/bridge releases include the simple-value check. Two live
  malformed probes closed cleanly with `1002` without harming a healthy peer or
  storing a variable. Seven Chrome acceptance checks and three live legacy/Close
  checks passed; health/readiness returned 200 and gauges returned to zero. The
  timestamped process/hash snapshot is
  `benchmarks/2026-10-15-cbor-retention-live.json`.
- The retention probe now supports synchronized producers sharing one unread
  destination, per-producer ordering checks, connected-idle observation, and
  automatic idle-close verification. Twelve fresh-server runs across 1, 8, and
  32 producers passed all delivery, counter, Close, and cleanup checks. Empty
  queues can coexist with enlarged receive capacity on still-open sessions;
  connection and input-size budgeting remain necessary alongside queue limits.
- Optional `--idle-timeout-secs` / `WAR_IDLE_TIMEOUT_SECS` retires joined sessions
  without completed application input/output, defaulting to disabled. Ping/Pong
  does not extend it; receive-only application output does. Queued/in-flight
  output retains the existing write/queue policy. Normal Close cleanup removes
  registrations and releases ordinary channel variables when the last member
  leaves. Decoder references release at transport end; published shared bodies
  keep their recipient-owned lifetime.
  Three real-socket regressions cover controls, active input/receive-only output,
  and unfinished fragmented input. All enabled retention cases observed exactly
  the expected 2/9/33 idle decisions and clean Close handshakes.
- The enabled-policy profile exposed per-message retiming: `Sleep::reset`
  accounted for 8.38% of sampled user cycles. The final schedule retains an
  already earlier alarm when activity postpones expiry, checks the live deadline
  on completion, and immediately retimes for an earlier heartbeat deadline.
  Exact expiry is preserved without coarse idle polling. The final enabled
  profile no longer listed reset among symbols at or above the 0.5% cutoff.
- Sixteen balanced twenty-second comparison cases passed. Final disabled-mode
  medians were 1,701,361 deliveries/s and 1,689 CPU ns/delivery, versus 1,610,854
  and 1,780 for `14b9529`. Final enabled-mode medians were 1,606,449 and 1,776,
  with the same 1.049 ms p99 bucket. Delayed rearming reduced enabled CPU by 2.0%
  versus the initial policy; its throughput ranges overlap, so the modest local
  result is not a general speed claim. Enabling the policy retains a measured
  cost against the new disabled default. Full ranges and methods are in
  `performance.md` and `benchmarks/2026-10-15-idle-final-cost.json`.
- Final retention checks passed at 1/32 producers and then 64 producers. The
  65-transport case verified all 4,105 data/barrier deliveries and 65 idle Close
  handshakes, with 8,388,648 encoded large-value bytes reused and 5,249 tiny bytes
  compacted. Sampled RSS fell from 269,212 KiB before acknowledgement to 6,560 KiB
  after retirement. Queue/connection/session/variable gauges returned to zero.
- The JS fixture now verifies six default-policy cases, restarts the temporary
  relay with idle expiry enabled, and verifies clean native/legacy idle Close.
  The native control test also verifies the 18-byte idle Close acknowledgement
  with a 17-byte application limit. Both heartbeat-only and combined idle/
  heartbeat tests confirm the earlier heartbeat deadline still wins.
- The current local native/bridge releases include the final schedule. The
  normal endpoints explicitly keep application-idle expiry disabled. Seven Chrome
  checks and three live legacy/Close checks passed, health/readiness returned
  200, and all gauges returned to zero. Process/hash and verification evidence is
  `benchmarks/2026-10-15-application-idle-live.json`.

## Exact verification boundaries

Unused enlarged native decoder storage now retires after five seconds without
large complete input, with `--read-buffer-idle-secs 0` preserving indefinite
reuse. It shares the existing pinned deadline wait, keeps clients connected and
published values in their original storage, defers busy frames, and preserves
fragment/control state. Three small backend hooks are vendored with original
package/commit checksums and the complete patch; CI audits all 61 package files
against the pinned archives plus that patch. The first-download and cached audit
paths both passed locally.

Eighteen matched connected-idle cases, a disabled-policy case and a 64-producer
control/default pair passed. At 32 producers the median last connected-idle RSS
fell from 140.24 to 12.02 MiB; the 65-connection pair fell from 262.93 to 12.43 MiB
without disconnecting clients. Sampled peak concurrent-input memory remained unchanged.
Small-population allocator variation and a roughly 2.6% observed large-payload
CPU cost are retained in `performance.md`, alongside the overlapping throughput
ranges and every raw record. Small-message traffic cost remained close to the
control, and 2,000 ordinary idle sockets added a median 20 KiB RSS versus control.

All 52 Rust checks, strict Clippy, seven JS socket cases and the original C++
46-case/eight-difference comparison passed with this version. The current local
native/bridge releases include the policy. A live five-second reset retained
both sessions and subsequent typed routing added no body copies; both closed
cleanly. Seven fresh Chrome checks and three live legacy/Close checks passed;
health/readiness returned 200 and final resource gauges were zero. See
`benchmarks/2026-10-15-read-buffer-idle-{build,summary,live}.json`.

The unchanged bundled C++ executable now runs with locally extracted, pinned
Debian TBB/OpenSSL libraries inside a private network namespace. Three fresh
process comparisons passed 46 common cases and eight explicitly asserted
differences per implementation: routing, byte/text and frame-size boundaries,
listeners, variables, fixed IDs, departure, and cleanup. Both implementations
acknowledged their eight remaining client closes; Rust's sessions, connections,
queue and variable gauges returned to zero with no write, Close, or transport
errors. The original's pre-GC membership/variable retention and several documented
behavior fixes were observed directly. See `compatibility.md` and
`benchmarks/2026-10-15-cpp-reference-{1,2,3}.json` for exact scope and provenance.
This compares the preserved executable using these host/local libraries; it
does not reconstruct its original operating system or measure C++ performance.
Native CBOR client tests use Node 24's real WebSocket;
the browser acceptance page has passed through the generated ES2022 module in
Chrome 155 on this Linux host. Other browsers and production bundler integrations
remain unverified. CI builds the bundle but does not run the browser acceptance.

Deployment examples are provided. The systemd unit was verified using the built
workspace executable substituted for its intended `/usr/local/bin/war-relay`
installation path. No system service or public endpoint has been installed.
Caddy is not installed here, so its example was not executed. Measurements are
local single-host plaintext sockets; no production or cluster claim is made.

## Decoder staging admission

The native relay now reserves decoder staging before allocating, in addition to
reclaiming unused storage. `--total-read-buffer-bytes` /
`WAR_TOTAL_READ_BUFFER_BYTES` defaults to 1 GiB; `0` disables admission. A bounded
16 KiB HTTP upgrade has temporary headroom, then separate frame/fragment leases
retain twice their requested capacity. Large frames include one charged read
chunk of read-ahead. Ordinary reads within the high water perform no shared
budget atomics or guard allocations. Refusals return HTTP `503` or a best-effort
WebSocket `1013`, before refused decoder growth. Frame admission precedes body
reads; fragment admission precedes appending the decoded frame. Reclaim/drop/
cancellation release the allowances; completed payloads retain their existing
shared owners.
This controls staging requests, not total process RSS or published body storage.

Three paired 64-source partial-frame runs with a 24 MiB allowance reduced median
held RSS from 148.22 to 8.14 MiB, while independent healthy routing and later
admission remained verified. The default admits most of that cohort; the memory
reduction belongs to the deliberately tighter configuration. Nine final pressure
cases, an eight-producer retention/reclamation case and over three million
selected-server churn lifecycles passed. Exact-size allocation was rejected
after repeated large-message CPU regressions; charged read-ahead recovered the
cost. Selected 1 KiB and 64 KiB CPU/throughput medians stayed within a few percent
of the prior release, with identical p99 bounds. Standing idle RSS increased
940 KiB at 2,000 sessions. See `performance.md` for all costs and limitations.

All 60 Rust tests, strict Clippy, formatting, seven JS socket cases and the
46-case/eight-difference original C++ comparison passed. Vendor verification now
covers 62 files against the original pinned packages plus the complete declared
patch. The host subsequently rebooted: saved runtime sources/releases matched
their recorded hashes; native/bridge/HTTP/Chrome were restarted, and all Rust,
Clippy/format/vendor checks, seven Chrome checks, three live legacy checks and
the five-second live reclamation/shared-routing check passed again. Final live
connection/session/queue/variable/staging gauges and error counters were zero.

At that checkpoint the native release hash began `a420b7a37a1d` and the bridge
began `d82df74b7cf8`. Exact hashes, sources, measurements and post-reboot live
evidence are in `benchmarks/2026-10-15-input-budget-{build,summary,live}.json`.

## Fragment capacity under pressure

A new real-socket check found that a valid 4 MiB message split into 3 MiB/1 MiB
fragments was refused under a 16 MiB staging allowance: geometric assembly
growth requested 6 MiB although the final length was known to be 4 MiB. The
decoder now bounds intermediate growth by the configured message ceiling and
final growth by the complete length, while retaining already sufficient capacity.
Actual-length validation still precedes allocation/admission. Text and binary
assembly preserve ownership and split UTF-8 state; unmetered behavior is retained.
No published application body moves as a result of this change.

Three alternating release pairs used the same frozen verifying client and the
normal 16 MiB message ceiling, eight producers, three 4 MiB waves and independent
healthy traffic. Peak staging allowance fell from 144.07 to 112.07 MiB (22.2%).
Median sampled peak server RSS fell from 90.08 to 77.50 MiB, with wide ranges;
no general RSS, CPU or throughput improvement is claimed. All 24 messages,
12 normal Close handshakes, per-wave resets and final resource release passed
in every run, with zero routed-body compaction. A 64-producer/one-healthy-client
case verified all 192 fragmented messages, 68 normal Close handshakes and all
three returns to the expected 532,480-byte connected allowance.

Fifteen complete-fragment workloads, including the expected old-release refusal,
and a 64-source partial-frame check passed. They cover the default five-second
reset, disabled reset/admission, the exact 64 KiB reset threshold and one byte
above it, tight staging admission, and the README command with a freshly built
client. All 65 Rust tests, strict Clippy, formatting, the 62-file vendor audit,
seven JavaScript socket cases and the 46-common-case/eight-explicit-difference
original C++ comparison passed. Sources, exact hashes, raw observations and
limitations are bound in `benchmarks/2026-10-16-fragment-final-{build,summary}.json`;
`performance.md` describes the measurements and earlier exploratory candidate.

The verified final native/bridge binaries replaced the quiescent post-reboot
processes. A fresh headed Chrome run passed all seven checks with no console
messages, and three live legacy/Close cases passed. Live decoder retirement
returned two connected sessions to their 16 KiB base after 5.019 seconds; typed
routing after retirement added no body copies. Health/readiness returned 200,
and final connection/session/queue/variable/staging gauges and error counters
were zero. `benchmarks/2026-10-16-fragment-final-live.json` records the process
identities, exact release hashes, browser report and live probe source.

At that checkpoint the native SHA-256 began `6f801ea16bdb` and bridge began
`933efb31e760`, using the same local endpoints listed below.

## Fragment scheduling

Valid messages with many empty continuations exposed a scheduling problem:
decoder reads could parse many frames before returning to the executor.
The native relay and legacy server now yield after at most 256 parsed frames
without a message. They resume the same incomplete data, UTF-8/control state
and admission leases; published CBOR values retain their shared ownership.
The asynchronous wrapper explicitly wakes itself so buffered input resumes
without requiring another network event. Library defaults remain unchanged;
the quota controls frame count per poll, without a time or total CPU guarantee.

Three alternating single-worker pairs at each read size verified 64 million
empty continuations, 32 complete messages and all five normal Close handshakes
per run. The median of maximum healthy RTTs fell from 10.662 to 0.871 ms with
4 KiB reads and from 322.164 to 0.864 ms with 128 KiB reads. Management RTTs also
fell. Fragment-heavy CPU increased about 1.7–1.8% in these medians. A separate
four-worker/16-source pair at each read size verified all 128 messages and
17 Close handshakes; maximum healthy RTT stayed below 0.8 ms on the candidate.
These are finite local results, without a production latency claim.

Twelve ordinary traffic cases verified 178,598,784 deliveries. The selected
1 KiB/64 KiB CPU medians changed by less than 0.6%, throughput by approximately
1% or less, with unchanged p99 bounds. The frozen traffic client intentionally
drops its TCP handles; every expected departure and final resource release was
accounted for, with normal Close verified independently. The new workload's
19 recorded cases also cover the README command, tight 16 MiB fragment staging
with the default five-second reset, and three reclamation waves with 64 sources
plus healthy traffic. All final staging/session/connection/queue/variable gauges
were zero; routed-body compaction stayed zero.

All 68 Rust tests, strict Clippy, formatting, the 62-file vendor audit and seven
JavaScript socket cases pass. The original C++ comparison now includes binary
and split-UTF-8 text messages with 1,024 empty continuations: all 48 common cases
and eight explicit differences pass. Three new decoder checks cover bounded
progress, explicit wakeup with entirely buffered input, cancellation/resumption,
UTF-8/control preservation, published payload pointers and terminal errors.

The verified release replaced the quiescent local native/bridge processes.
A fresh Chrome run passed all seven checks with no console messages, three
live legacy/Close cases passed, and live decoder retirement returned two
connected sessions to 16 KiB after 5.040 seconds. Subsequent typed routing added
no body copies. Health/readiness returned 200; final resource gauges and error
counters were zero. Exact sources, hashes, counters, workload traces and live
process identities are bound in `benchmarks/2026-10-16-fairness-{build,summary,live}.json`.
See `performance.md` for cost and measurement limits.

At that checkpoint the native SHA-256 began `8bfeb5be88d2` and bridge began
`17e21d754a6b`, using the same local endpoints listed below.

## Rejections and interrupted input (current checkpoint)

A failing socket regression exposed silent native drops after HTTP upgrade but
before channel join for malformed WebSocket frames and excessive message sizes.
Native input errors now share one Close mapping across both phases: `1002` for
protocol errors, `1009` for size, and `1013` for staging exhaustion. Header-based
refusals do not wait for the body. `war_join_rejected_total` records each
application rejection decision before registration, excluding HTTP admission,
peer-Close paths and service shutdown. Existing joined-session counters retain
their scope. Successful forwarding and shared body ownership are unchanged.

The independent raw corpus covers 776 input/context cases over 2,311 exchanges:
accepted and rejected CBOR, routed/different-target/variable contexts, mixed
fragments crossing 257 empty continuations, interleaved Ping/Pong, split writes,
and pipelined join/data. One-worker/one-producer, four-worker/eight-producer and
the default command all pass. Rejected data never reaches a witness or mutates
its variable sentinel; encoded valid values stay byte-identical. Exact stage,
protocol, join, message, byte and delivery counts match. Each full run verifies
2,204 complete-message policy Close handshakes and 45 terminal-decoder Close
receipts; the latter do not claim acknowledgement. Five library-normalized
peer-Close cases are tracked separately. All final resource gauges and
unexpected errors are zero. The control reproduces 18 silent pre-join drops;
the final release reports the correct Close in every corresponding case.

New socket checks hold unfinished messages through 513 empty continuations and
a Pong, then verify join timeout with 75 healthy round trips, admin cancellation
with immediate membership/queue retirement, and shutdown before/after join.
All decoder reservations release. Stage/rate/staging counts are exact; HTTP
`503` admission and normal peer Close do not inflate the new counter.

All 73 Rust tests, strict Clippy, formatting, the 62-file vendor audit, seven JS
socket cases and the 48-common-case/eight-explicit-difference C++ comparison pass.
The complete corpus is now a GitHub CI step; its equivalent debug command passes
locally, with no hosted CI run claimed. Twelve alternating traffic checks show
throughput medians within 0.7% and CPU medians within 1.3% of the prior release;
no speed improvement is claimed. The prior incomplete-frame pressure check also
passes with exact expected departures and zero final reservations.

The new native/bridge binaries replaced the quiescent local services. A deliberate
live pre-join framing error received `1002` and incremented the new counter from
zero to one. Fresh Chrome checks (seven), live legacy checks (three), health and
readiness pass. Default decoder retirement returned two connected sessions to
16 KiB after 5.033 seconds; subsequent typed routing added no body copies.
Final resource gauges and unexpected errors are zero; telemetry retains the one
intentional rejection. Sources, hashes, exact counters and live process identities
are bound in `benchmarks/2026-10-16-input-rejection-{build,summary,live}.json`.

Current local endpoints are native `ws://127.0.0.1:1338`, optional bridge
`ws://127.0.0.1:1337`, management `http://127.0.0.1:1339`, and browser check
`http://127.0.0.1:4180/`. Native SHA-256 begins `0bafc0d405fc`; bridge begins
`16ec70ea7efa`. Application-idle Close is disabled for manual testing; normal
five-second retirement and 1 GiB staging admission remain enabled. The ongoing
goal stays active; no hosted deployment or cluster capacity is claimed.

## Latest checkpoint: browser-origin admission, 2026-10-16

Native and legacy public listeners now share `--allowed-origin` /
`WAR_ALLOWED_ORIGINS`: exact browser page origins, at most 64 entries, with
unrestricted defaults. Unlisted, empty, combined and duplicate Origin headers
receive HTTP `403` before upgrade. Missing Origin remains allowed for native
and bridge clients; `null` requires an explicit entry. This is a browser boundary,
separate from authentication and the preserved global/private routing scopes.
The policy borrows headers/configuration once per upgrade and adds no per-message
checks or application-body movement. The bridge checks its own ingress and now
shares the native 16 KiB HTTP reader bound; its later 64 KiB test frames still
route correctly. Existing `legacy_bridge::serve` callers retain unrestricted
behavior, with a separate opt-in library entry point.

All 80 Rust tests pass, including new HTTP/capacity/counter and legacy boundary
regressions. The policy checks also pass in release. Strict Clippy, formatting,
the 62-file vendor audit, seven JS socket cases, the complete independent
776-case/2,311-exchange corpus in release and the local debug CI command, and the
48-common-case/eight-explicit-difference C++ comparison pass. Actual Chrome 155
passes seven CBOR checks from an allowed page and preserves legacy binary/text
delivery with clean Close. The same page from another port receives `403` on
both guarded listeners. Native denial increments only its intended counters;
bridge denial creates no upstream connection. All final resource gauges are zero.

Twenty-six sequential cost runs pass: 15,148,001 complete connection lifecycles
and 41,517,392 verified traffic deliveries. The longer unrestricted comparison
differs by −0.12% median churn throughput and +0.09% server CPU/lifecycle; p99
stays in the same histogram bucket. The short enabled 64-entry list costs +1.26%
median CPU/lifecycle against its control. Ordinary 1 KiB traffic medians differ
by −0.69% throughput and −0.28% CPU/delivery, with the same p99 bucket. No routed
body compaction occurs in those cases. The earlier short unrestricted spread
and the longer confirmation are both retained; no general speed claim is made.

The quiescent old native/bridge processes were verified by PID, cwd and executable
digest before SIGTERM, and both exited zero. Their replacements run at the same
local endpoints, with Origin unrestricted for manual testing. Fresh Chrome checks
(seven), live legacy/Close checks (three), liveness and readiness pass. The normal
decoder retirement returns two connected sessions to 16 KiB after 5.020 seconds;
later typed routing adds zero body copies. Final resource and unexpected-error
counters are zero. The separate guarded-browser fixture was stopped cleanly.

Current native SHA-256 begins `2849ab156586`; bridge begins `546d7e85772b`.
Native WebSocket is `ws://127.0.0.1:1338`, legacy is `ws://127.0.0.1:1337`,
management is `http://127.0.0.1:1339`, and the browser check remains at
`http://127.0.0.1:4180/`. Application-idle Close is disabled for manual testing;
five-second decoder retirement and 1 GiB staging admission stay enabled.
The [summary](benchmarks/2026-10-16-origin-summary.json),
[manifest](benchmarks/2026-10-16-origin-build.json), and
[live record](benchmarks/2026-10-16-origin-live.json) bind sources and observations.
The ongoing goal remains active, with no hosted deployment, cluster capacity or
TLS-proxy acceptance claimed.

## Latest checkpoint: ID registry lifetime, 2026-10-16

An unchanged-release probe reproduced slower global routing after 8,192 peers
had joined and left separate rooms. The two original global clients stayed
connected; their ID map retained spare buckets from the large population.
A separate user-cycle profile and annotated group-scan loop identified this
work inside routing. The selected change reduces excess ID-table capacity after
session removal and occupied fixed-ID takeover, keeping small tables and
regrowth headroom. It adds no per-message check, allocation or body movement.
Peer and payload pointers, in-flight byte reservations and late displaced-owner
cleanup are verified directly. Other registry tables keep their existing policy.

Three final processes per build, each with three 8,192-peer waves, verify
31,309,200 ordered deliveries and 147,468 normal Close handshakes. Post-wave
medians improve from 416,192 to 447,340 deliveries/s (+7.48%) and from 1,638
to 1,464 server ns/delivery (−10.64%), with the same p99 bucket. Fresh CPU differs
by +0.62%. Filling/draining stay in the same 10 ms CPU bins; rehash allocation
and brief write-lock work remain a tradeoff. Two separate 2,250-sample profiles
have zero lost samples; `Relay::receive` drops from 23.97% to 7.91% of sampled
user cycles. Profiled timings are excluded from comparisons.

All 26 ordinary cost cases pass: 63,091,904 traffic deliveries and 2,531,922
normal churn lifecycles, with zero routed-value compaction and complete cleanup.
At 1 KiB, median throughput/CPU differ by +0.81%/−0.40%; churn differs by
+0.07%/+0.12%. A short 64 KiB tail spread received four longer alternating pairs:
median p99 is then 4.194304 ms in both builds, throughput differs by −1.57%, and
CPU/delivery by +0.27%, with overlapping ranges. One selected long run retains
the higher 8.388608 ms bucket. The gain is scoped to routing after population
waves; these data do not establish a general ordinary-traffic speed improvement.

All 83 Rust tests pass, with the three new regressions also passing in release,
plus strict Clippy/formatting, 62-file vendor verification, seven JS socket cases,
the independent 776-case/2,311-exchange release corpus and the original C++
48-common-case/eight-explicit-difference comparison. A four-worker network
regression keeps exact private delivery active during 2,048 concurrent arrivals
and departures. The new probe also checks normal service exit; a real-socket
wrapper that deliberately exits 7 reproduces the earlier verifier's false
success and is correctly rejected by the final runner. All captured population-comparison
services independently exit zero; calibration is excluded from timing results.

Quiescent native/bridge services were verified by PID, cwd and executable hash
before SIGTERM, and both exited zero. The replacements pass a live 1,024-peer
wave: typed global delivery remains exact after retirement, all 1,026 peers
close normally and no additional body copies occur. Default decoder retirement
returns two connected sessions to 16 KiB in 5.025 seconds; subsequent typed
routing again adds zero copies. Seven fresh Chrome checks and three named live
legacy/Close cases pass. Health/readiness return 200; final resource gauges and
unexpected-error counters are zero.

Current native SHA-256 begins `579dc1751408`; bridge begins `5d3460dbdf5a`.
Native remains `ws://127.0.0.1:1338`, legacy `ws://127.0.0.1:1337`, management
`http://127.0.0.1:1339`, and browser check `http://127.0.0.1:4180/`. Origin stays
unrestricted and application-idle Close stays disabled for manual testing;
normal decoder retirement and 1 GiB staging admission remain enabled.
The [summary](benchmarks/2026-10-16-registry-summary.json),
[manifest](benchmarks/2026-10-16-registry-build.json), and
[live record](benchmarks/2026-10-16-registry-live.json) bind the observations.
This is finite local evidence; larger active fanout, other table scan costs,
multi-hour/TLS/proxy paths and cluster operation remain unmeasured. The ongoing
goal stays active, with no hosted publication or deployment claimed.

## Latest checkpoint: room membership lifetime, 2026-10-16

Measured retained ordinary/global membership tables after departure. A 4,096-peer
case showed little CPU change; an 8,192-peer room case rose from 1,457 to 1,688
native ns/delivery, and private global-index delivery from 1,476 to 1,714.
A separate profile identifies empty-bucket scanning. One shared typed helper
now trims sparse peer indexes, retaining small tables and regrowth headroom.
It adds no per-message check, allocation or body movement. Ordinary empty rooms
retire directly; global variables retain their persistent lifetime.

Twelve final processes verify 30,873,216 application deliveries, 402,800,640
disconnect deliveries and 98,328 normal Close handshakes. Each keeps two original
clients through 8,192 transient members. Every remaining reader checks each
departure's exact FIFO prefix; 16-close windows preserve default 64-message
queues and all notifications. Post-wave room medians improve from 403,031 to
440,729 deliveries/s (+9.35%) and 1,696 to 1,488 server ns/delivery (−12.28%).
Private global-index medians improve from 409,571 to 434,403 deliveries/s (+6.06%)
and 1,636 to 1,494 ns/delivery (−8.68%). Both median p99 buckets move from
0.065536 to 0.032768 ms. Fresh CPU differs by −0.64%/+1.44%; filling/draining
and profile timings stay separate. Rehash allocation and brief write locking
are tradeoffs. The prior global-ID probe also passes three further waves.

All 84 Rust tests pass; all three sparse-index regressions pass in release,
alongside strict Clippy/formatting, vendor verification (62 files), seven JS
socket cases, the 776-case/2,311-exchange release corpus and the C++ comparison's
48 common cases/eight explicit differences. A new regression holds a payload
and variables through 1,024 administrative departures in ordinary/global rooms,
checks peer/variable/payload pointers and later routing, then verifies held
reservations and the two variable lifetimes. All eighteen ordinary cost guards
pass; CPU medians differ by +0.23% at 1 KiB, −0.71% at 64 KiB and +1.07% for
churn, with matching p99 buckets and overlapping ranges. Gains remain scoped
to routing after population waves.

Old quiescent services were verified by PID, cwd and executable SHA before
SIGTERM; both exited zero. Updated live room/global waves verify 1,051,648
disconnect deliveries, 130 typed messages during/after departure and 2,052 clean
closes, with zero additional body copies. Ordinary variables survive retirement;
the global live probe writes no persistent variables. Default decoder retirement
takes 5.016 seconds with two sessions connected, and later routing adds no copies.
Seven fresh Chrome checks and three named legacy/Close cases pass. Health and
readiness return 200; resource gauges and unexpected-error counters are zero.

Native SHA-256 begins `c276a8c1172d`; bridge begins `45d309801638`. Endpoints
remain native `ws://127.0.0.1:1338`, legacy `ws://127.0.0.1:1337`, management
`http://127.0.0.1:1339`, and browser `http://127.0.0.1:4180/`. Origin remains
unrestricted and application-idle Close disabled for manual use; decoder
retirement and 1 GiB input staging remain enabled. The
[summary](benchmarks/2026-10-16-member-summary.json),
[manifest](benchmarks/2026-10-16-member-build.json), and
[live record](benchmarks/2026-10-16-member-live.json) bind sources and observations.
Unpaced departure, larger active fanout, other table costs, long/TLS/proxy paths
and clustered operation remain unmeasured. The continuing goal stays active.

## Latest checkpoint: channel-name lifetime, 2026-10-16

Measured the remaining channel-name index after temporary rooms retire. The
first 8,192-room socket run rises from 1,945 to 2,054 native ns per authenticated
channel reply. A separate profile shows the empty-bucket scan; a regression
fails on retained capacity before the change. The existing bounded metadata
helper now also trims channel names on removal of an empty ordinary room.
Small tables and regrowth headroom remain. Names, variables, peers, queues and
published application storage retain their addresses; normal messages add no
work or allocation.

Six final processes keep two original global-room clients through two waves of
8,192 temporary ordinary rooms each. Every CBOR query reply is checked against
independent literal bytes. Gated HTTP snapshots check every channel field and
base64 ordering at fresh, peak, retired and empty states. All 33,165,328 queries
and 98,316 normal Close/EOF handshakes pass with exact counts/bytes, zero routing
body compaction and full cleanup. Median post-wave CPU improves from 2,047 to
1,951 ns/query (−4.71%), throughput from 442,038 to 470,346 queries/s (+6.40%),
and p99 from 0.065536 to 0.032768 ms. Fresh CPU is +0.88%; filling/draining
CPU medians remain 0.34/0.19 seconds across two waves. Profiles are diagnostic;
the finite improvement applies to queries after room waves.

All 85 Rust tests pass, including concurrent private routing through 2,048
arrivals/departures. All four sparse-index regressions pass in release, plus
strict Clippy/formatting, 62-file vendor verification, seven JS socket cases,
the 776-case/2,311-exchange release corpus and C++ 48-common/eight-difference
comparison. The new unit regression verifies binary channel names, both distinct
inspection sort orders, retained key/value/peer pointers, an in-flight payload,
ordinary-variable removal on rejoin and global-variable persistence.
Eighteen ordinary cost cases pass with matching p99 buckets and overlapping
throughput ranges; CPU changes are +0.56% at 1 KiB, +0.19% at 64 KiB and +0.13%
for churn. The old global-routing probe also passes three further waves.

The stability runner now waits for native service exit before reporting success,
records its status/forced-kill flag and retains shutdown logs. A real-socket
wrapper exits 7 after its native child exits zero; this reproduces false success
before the verifier fix and is rejected afterward. All eighteen cost services
exit zero without forced killing. A diagnostic verifier initially rejected
`perf`'s expected SIGINT stop status; that failure is retained separately and
excluded from timings. A fresh completed recording passes with the corrected
stop-status check. The timed runner snapshot is preserved.

Quiescent prior services were checked by PID/cwd/executable SHA and both exited
zero. Updated manual services pass a 1,024-distinct-room wave with 65 typed
deliveries during/after departure, 1,029 clean closes and zero additional body
copies. Binary/ascii room variables survive the wave and are absent after
rejoin. Default decoder retirement takes 5.027 seconds; typed delivery afterward
adds no copies. Fresh Chrome and three named legacy/Close cases pass. Health,
readiness and browser return 200, with final resource gauges and unexpected
error counters zero.

Native SHA-256 begins `8903af76ff2d`; bridge begins `5cb66b8504dc`. Endpoints stay
native `ws://127.0.0.1:1338`, legacy `ws://127.0.0.1:1337`, management
`http://127.0.0.1:1339`, browser `http://127.0.0.1:4180/`. Origin stays unrestricted
and application-idle Close disabled for manual use. The
[summary](benchmarks/2026-10-16-channel-summary.json),
[manifest](benchmarks/2026-10-16-channel-build.json), and
[live record](benchmarks/2026-10-16-channel-live.json) bind sources and observations.
Large active inspection/lock contention, unpaced departure, long duration,
TLS/proxies and clusters remain unmeasured. The ongoing goal stays active.

## Latest checkpoint: concurrent inspection, 2026-10-16

Channel inspection now captures coherent bounded names and numeric metadata
under the registry read lock, then releases it before formatting, sorting,
encoding and enqueueing. CBOR captures only names/member counts; HTTP also
captures variable usage. Inline names avoid per-name reference counts. No peers,
variable values or application bodies enter the snapshot, and normal forwarding
adds no work or allocation. A regression proves the registry remains writable,
metadata stays stable through mutation/final destruction and a held routed body
keeps its original pointer/reservation until drop.

The final eighteen fresh-process cases keep 8,192 single-member rooms alongside
two private pairs and eight reconnect workers. Optional CBOR/HTTP inspectors
complete exactly 200 full queries/s. Every name, count, metadata field, ordering,
application frame, normal Close/EOF and aggregate count/byte total is checked.
All 71,136,608 healthy deliveries, 6,832,194 reconnect lifecycles, 19,200 timed
inspections and 6,979,734 normal closes pass, with zero routed-body compaction,
complete resource release and service/client exit zero.

CBOR-case healthy throughput improves 6.98%, reconnect throughput 6.34%; HTTP
improves 17.68%/10.66%. Healthy p99 upper buckets improve 0.262144→0.131072 ms
for CBOR and 1.048576→0.262144 ms for HTTP; reconnect HTTP p99 halves. HTTP-case
healthy p50 worsens one bucket, and total CPU rises with more completed work.
No-inspection throughput ranges overlap with matching p99 buckets. The first
shared-name prototype and diagnostic profiles remain separate from the final
compact representation. Full methods, tradeoffs and raw evidence are in
[performance.md](performance.md#concurrent-channel-inspection-2026-10-16).

Twenty-four ordinary guard cases pass. The first 64 KiB guard shows a worse
p99 bucket, −4.18% throughput and +2.47% CPU per delivery. A separate identical
six-case follow-up changes direction (+3.99%/−2.88%) and matches p99 buckets;
both original and follow-up remain recorded. These short runs do not establish
a consistent ordinary large-message effect. Inspection-specific benefits and
their latency/CPU tradeoffs remain the supported result.

A direct comparison now uses the original shipped C++ executable and a separate
four-worker source build. The shipped artifact has 31 failed listener binds
and one active loop here; one-core matched results are 742,524 versus 583,047
private deliveries/s for C++/Rust. The rebuilt C++ uses upstream optimization
flags/current system libraries, with only the worker-count expression changed
in a temporary source copy. Four listener sockets/six threads are verified in
every fresh process. Three repetitions yield 1,110,733 versus 615,940 private
deliveries/s, 1,074 versus 1,449 CPU ns/delivery, and 20.49 versus 101.10 MiB
ready RSS at roughly 8,200 sockets. C++ retains a clear forwarding/memory
advantage in this workload. Query-case medians are 686,055 versus 630,576;
C++ varies from 681,759 to 1,085,770, which remains explicit in the record.

The rebuilt four-worker C++ churn case fails three diagnostic checks: timeout,
self-delivery mismatch and repeated welcome ID before its first GC. Those
failures have no valid throughput result. Successful single-listener churn
results remain distinct from multicore source behavior. The twelve successful
rebuilt/Rust cases verify 76,130,992 healthy deliveries, 9,600 timed inspections
and 98,376 normal closes. The preserved-binary one/four-core-allowance matrices
add 36 passing cases. This compares complete binary/CBOR implementations and
their recorded build configurations, rather than isolating language overhead.
See the [rebuilt comparison](performance.md#rebuilt-four-worker-c-source).

All 86 Rust tests, all 29 release library tests, strict Clippy/formatting,
62-file vendor verification, seven JS socket cases, the 776-case/2,311-exchange
release corpus and the 48-common/eight-difference original-runtime comparison
pass. The latest HTTP workload client smoke also passes after adding native
protocol adapters and diagnostic ID tracking. Audited fault runs are separate
from normal performance timings.

Quiescent prior manual services exited zero. Updated native/legacy services
pass the 1,024-room wave with 65 typed deliveries, 1,029 clean closes and no
extra body copies; default decoder retirement takes 5.022 seconds. Fresh Chrome
passes all seven browser checks, and the three named live legacy/Close cases
pass. Health/readiness/browser return 200 and final resources/errors clear.
Native SHA begins `2c6f670c1215`; bridge begins `d1187db67980`. Endpoints remain
native `ws://127.0.0.1:1338`, legacy `ws://127.0.0.1:1337`, management
`http://127.0.0.1:1339` and browser `http://127.0.0.1:4180/`, with unrestricted
Origin and application idle expiry disabled for manual use. The
[summary](benchmarks/2026-10-16-inspection-summary.json),
[manifest](benchmarks/2026-10-16-inspection-build.json) and
[live record](benchmarks/2026-10-16-inspection-live.json) bind the checkpoint.
The continuing improvement goal remains active.

## Next useful iterations

1. Extend the independent original-runtime comparison when routing, listener,
   variable or ID-management behavior changes. The current 48-case corpus and
   eight explicit difference checks now have real C++ wire evidence; longer
   scheduling windows, adverse networks and its garbage-collection cycle remain
   outside that finite comparison.
2. Profile private-forwarding CPU and idle-connection allocations against the
   current direct C++ result before changing counters, queue representation or
   read storage. The readiness replacement retains shared bodies and now uses
   less RSS at roughly 8,200 sockets. Its four-worker private CPU per delivery
   remains higher than rebuilt C++, while its one-core private and churn
   workloads improve on the shipped executable. Measure more active flows and
   worker placement before treating two-pair throughput as a scaling ceiling.
   The 64-pair extension now supplies a stronger verified workload and separate
   profile, but does not saturate the four-core allowance. Padded-global and
   striped-cumulative counter experiments do not establish a consistent benefit;
   preserve the existing global atomic metrics and reservation checks.
   The ID ownership map now shrinks after substantial retirement, with measured
   global-routing recovery after large population waves. Membership indexes now
   have ordinary-room and global-index private-delivery measurements. Channel-name
   queries after temporary-room waves and full inspection at 8,192 stable rooms
   with concurrent traffic/churn are now measured too. Inspection captures
   bounded metadata under the lock and sorts/encodes after release. Larger
   populations, higher inspection rates and snapshot/output allocation costs
   remain useful measurements before adding pagination, caching or sharding.
   Concurrent retention, staging pressure and delayed storage reclamation now
   cover up to 64 producers plus independent healthy traffic across repeated
   waves. Process-wide reservations precede decoder allocations and cover
   incomplete frames plus fragment assembly; final-fragment growth uses its
   known complete length. Empty-continuation parsing now yields cooperatively;
   mixed-fragment/CBOR rejection and cancellation/deadline cases now run with
   concurrent healthy traffic. Extend partial-frame and blocked-output schedules,
   duration and populations when measurements justify the next change. Preserve shared bodies,
   exact peer Close acknowledgements, and immediate dropping of partial output.
3. Extend duration, fanout, adverse networks and connection mixes beyond the
   verified 100,000-user small-room workload. The new 65-second heartbeat/churn/
   unread checks and complete population waves establish finite local capacity;
   multi-hour operation, mass unread recipients, larger broadcasts and TLS/WAN
   behavior remain unmeasured at that population. The overload diagnostic is
   predominantly system CPU. Profile kernel work, scheduling/locality and
   higher-rate traffic before selecting a broader routing/index change. Keep
   exact global budgets and the current ownership model until a measured
   bottleneck justifies another representation or sharding.
4. Extend browser acceptance across engines and production bundlers. Broaden the
   deterministic CBOR/fragmentation corpus into reproducible stateful operation
   sequences and paced transport failures, preserving resource bounds, exact
   counter expectations and source fidelity. The current finite corpus is not
   exhaustive fuzz coverage.
5. Extend connection-admission policy for public multi-application operation
   and tenant boundaries with an explicit compatibility and trusted-proxy model.
   Optional direct-peer IP quotas now cover both public ingresses; broader
   address populations and connection-rate workloads remain useful measurements.
   Optional browser-origin admission also covers both public
   ingresses; the existing global/private scopes intentionally match WAR.
6. Design node ownership and cross-node routing only after single-node capacity
   and operational needs are established. No replicas are currently a cluster.
