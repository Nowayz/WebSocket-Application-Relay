# WebTransport pivot — implementation and acceptance ledger

Status: **in progress, performance parity not established**. This branch starts at
`a087cea` (the verified concurrent-listing and observer-barrier repair). The original
working checkout's unrelated changes are untouched. The frozen WebSocket baseline
and prior benchmark receipts remain under `artifacts/acceptance-fanout/`.

## Required outcome

Replace the production data transport with browser WebTransport over HTTP/3 while
retaining CBOR values, IDs, room/global/private routing, authentication, variables,
inspection, admission, observer semantics, and orderly cleanup. Make unreliable
messaging an explicit API choice. Demonstrate benefits and equal or superior
performance with equivalent workloads; do not count lost messages as throughput
or present lower reliability as an equivalent reliable workload.

## Wire contract

The endpoint is `https://host:port/war`, with real TLS verification. Local development
may use `--dev-cert` and the logged public SHA256 certificate pin. Production uses
`--tls-cert` / `--tls-key`. UDP reachability is required.

Each session opens a bidirectional reliable stream. Records are a four-byte unsigned
big-endian length followed by exactly one existing CBOR message. The first record
is the existing room subscription. Welcome IDs, commands, command replies, reliable
messages, and departure notifications retain the existing CBOR representation and
FIFO stream ordering. Large values continue on the stream without application-level
fragmentation protocols. An incomplete stream record is a session error.

Each datagram contains one complete existing `[target, value]` CBOR route. Routing
retains its unreliable attribute through private/room/global/observer delivery.
Target zero (commands) is forbidden on this lane. The server never upgrades an
unreliable message to a reliable one. Oversized recipient datagrams are dropped;
the negotiated recipient MTU can differ from the sender's. There is no delivery,
ordering, or cross-lane barrier promise for datagrams already in the network.
Applications that need stale-state suppression should include a sequence or version
in their value. Reliable observer barriers retain their existing contract.

The client exposes `sendUnreliable` and `broadcastUnreliable`; message events add
`unreliable: true` only for datagrams. Existing reliable message event shapes remain
unchanged. HTTPS URLs select WebTransport. Explicit WS URLs / injected WebSocket
classes and `--legacy-websocket` retain a comparison path during migration; there is
no automatic downgrade from a failed WebTransport connection.

## Design constraints and remaining acceptance work

- Reuse the routing core rather than a parallel copy of room or command logic.
- Keep bounded admission, byte accounting, deadlines, worker placement, and cleanup
  effective for both application storage and the new QUIC transport buffers.
- Bounded continuation scheduling and retirement wakeups are implemented and a
  1,025-recipient real-QUIC fanout/departure test passes. Add forced cancellation
  and observer-barrier tests before claiming full equivalence.
- Datagram submissions/drops/oversize now have separate counters. Finish QUIC
  buffer accounting; local enqueue is not proof of remote datagram delivery.
- Verify browser certificate handling, reliable large files, datagrams, observer
  barriers, origin rejection, overload, rate limits, idle deadlines, and shutdown.
- Port maintained benchmark/acceptance clients and deployment instructions; a small
  loopback probe is diagnostic evidence, not the full acceptance suite.
- Compare matched private/fanout/mixed/100k workloads with verified received counts,
  CPU, RSS, latency distributions, setup and retirement costs. Include encrypted
  controls so QUIC's mandatory encryption cost is distinguishable from protocol cost.
- Exercise controlled loss/reordering and mixed bulk/state traffic. Report freshness,
  delivered goodput, losses, and reliable progress separately.
- Update public README/protocol/operations/client docs only with verified final
  behavior, and rerun product checks plus actual browser acceptance.

## Sources

- [MDN WebTransport datagrams](https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/datagrams)
- [wtransport implementation and API](https://docs.rs/wtransport/0.7.2/wtransport/)

Datagrams are unordered and unreliable; streams provide reliable ordered bytes.
These properties motivate the two explicit lanes, not an assumed throughput win.

## First verified checkpoint (2026-10-21)

The initial release binary passed the actual Chromium 156 browser check with pinned
TLS: three sessions, typed broadcast, cross-room private delivery, explicit datagram
metadata, 128 KiB and 5 MiB reliable values, concurrent variables, exact IDs, departure
notification and closure. Post-check sessions/connections/queued/variable bytes were
zero. The screenshot and machine-readable browser result are in
`output/playwright/webtransport-initial.png` and
`artifacts/webtransport/browser-check.txt`.

After adding fair continuations and input admission, 159 library tests passed (one
ignored), and three native HTTP3 integration tests passed, including 1,025 recipients
and their departure notifications. Clippy with warnings denied passed. These are
specific checks, not the complete migrated acceptance suite.

The initial diagnostic used 32 clients, 1,024-byte application payloads, one in-flight
self-route per client, three seconds per run, three alternating-order runs per mode.
Server CPUs were 0–3; client CPUs 8–11. Each response was checked byte-for-byte. Values
below are medians, and latency values are directly measured, not histogram bounds.

| Mode | Deliveries/s | RTT p50 us | RTT p99 us |
| --- | ---: | ---: | ---: |
| Existing plaintext WebSocket path | 641,927 | 47.989 | 95.637 |
| WebTransport reliable stream | 523,949 | 58.678 | 173.514 |
| WebTransport datagram | 449,546 | 68.568 | 202.493 |

**Parity is not established.** This probe lacks an encrypted WebSocket control,
full fanout/mixed/capacity coverage, and impairment/freshness tests. It is a guide to
profiling, not final acceptance. The initial executable also predates subsequent
fairness, input-budget and metric changes; it is frozen with hashes under
`artifacts/webtransport/binaries/`. Raw samples, command runner, errors encountered
while developing the probe, and logs remain in `artifacts/webtransport/`.

Initial fixture issues were corrected before the nine reported samples: the probe
first shared a room (departure notices could be mistaken for data), then failed to
handle WebSocket Ping frames. The final probe isolates client rooms and handles Ping.
The browser bundle builds correctly with piped stdout; redirecting esbuild-wasm's
Node output directly to a regular file exposed an upstream loader recursion error.

## First profile and rejected write experiment

Privileged read-only perf captures of the initial executable (199 Hz, separate
8-second runs) are retained in `artifacts/webtransport/profile-*.data` and `.txt`.
No host performance or security settings were changed. In the datagram capture the
WebTransport driver's dispatch loop accounted for 6.86% self samples, clock reads
4.01%, application session polling 2.43%, and AES-GCM encryption/decryption about
3.1% combined. Sampling does not establish how much speedup removing any one cost
would yield. The driver source forwards each datagram through an additional task,
a one-slot channel, and an async mutex; a direct single-session QUIC adapter is a
candidate for a separately measured experiment.

A zero-copy QUIC chunk-write experiment passed native correctness but did not show
a compelling whole-operation win. Three matched runs per size compared checkpoint
`9f82ba0` with only the output writer changed; clients were frozen. At 1 KiB,
median throughput was 523,881 versus 530,875/s (+1.3%), with p99 about 171 us for
both. At 64 KiB it was 35,367 versus 35,727/s (+1.0%), with p99 about 27 ms and
median peak server RSS increasing from 17,508 to 19,296 KiB. The experiment was
**reverted**, rather than adding complexity on this evidence. The patch, binaries,
all twelve raw samples, CPU seconds and RSS are retained under
`artifacts/webtransport/chunks-*` and `binaries/`.

## Maintained-client and lifecycle checkpoint

Reliable framing now reserves an empty record for graceful close. After a joined
client sends that record, the server stops admitting input, transfers cancelled
publications to retirement, removes the registration, drains accepted reliable
output, and replies with an empty record. The client reads that acknowledgement
before closing QUIC. Local QUIC close alone does not prove server-side retirement.
Before joining, an empty record remains an invalid subscription. This transport
exchange does not add or change CBOR commands.

Application idle deadlines reset on application input or submitted output. QUIC
keepalives and idle detection replace transport Ping/Pong policy. Rate admission is
shared across the reliable and unreliable lanes. Read reservations, origin rejection,
connection admission, graceful close ordering, and idle refresh/expiry are covered
by eight real HTTP3 integration tests. Three JS stream tests exercise split/coalesced
records, truncated records, datagram limits, and a real acknowledgement dependency
for graceful close (using a deterministic stream fixture).

The maintained inspection and capacity clients now select HTTPS WebTransport with
a verified certificate pin. Their traffic generation, full payload assertions,
per-user checks, and exact resource/counter checks remain shared with the WebSocket
path. Capacity checks use the endpoint's independent QUIC connection census instead
of assuming one TCP socket per session. The application connection/session gauges
are checked separately. One UDP socket can serve many QUIC connections.

A maintained mixed smoke run passed with 16 stable rooms, four private pairs, 64
fanout readers, one CBOR inspector and one churn worker. It checked 1,532,684 delivered
messages during the measurement and completed 3,242 churn lifecycles. A subsequent
10,000-session capacity run passed every gate: 30,000 paced routing checks, RTT p99
bound 0.262144 ms, maximum 0.641324 ms, and zero sessions/connections/queued resources
at final cleanup. Server RSS at the measured gate was 762.77 MiB; memory use is a
material regression against the WebSocket architecture. These are bring-up/correctness
runs, not final matched performance acceptance.

The first 100,000-session attempt failed before its connected gate with HTTP3 error
261 (`H3_FRAME_UNEXPECTED`). It is recorded as a failure, not capacity evidence. Stack
inspection found that wtransport's driver cancels a control-stream future when other
branches become ready, while the nested frame reader keeps partial frame state in
that cancelled future. This is a plausible mechanism requiring a controlled check;
the failure alone does not prove causation. A single-session adapter that retains
control-read state and avoids the additional datagram relay task is the next candidate.

## Direct HTTP3 stack and shared-socket buffer investigation

The selected candidate uses `web-transport-quinn 0.13.3` for the single HTTP3 session
and its stream/datagram I/O. It retains `wtransport` for certificate/configuration
helpers and independent interoperability clients. The relay's room/ID/control logic
is shared as before. Eight native transport tests and the Chromium 156 browser
check pass on this candidate, including the 5 MiB file and graceful-close exchange.
The complete Rust all-target run passes 259 tests (one ignored); Clippy with warnings
denied and TypeScript pass. Browser completion also leaves the independent QUIC,
application connection/session, queue, input reservation and variable gauges at zero.

Both an old-client interoperability run and a direct-client run completed 100,000
sessions with the new server. They exposed a separate queueing problem: one UDP socket
now aggregates traffic from the entire population, while its OS default receive
buffer was sized like one ordinary socket. QUIC recovered the dropped packets, but
the nominal four-second phase stretched to almost eight seconds. Delivery/count
success alone therefore did not establish offered-load performance.

The controlled comparison uses the **same server/client executables** and changes
only the shared UDP receive-buffer request at both endpoints. Native clients and the
server expose a zero (OS default) setting and a 4 MiB request. The latter is a measured
burst allowance, not a message/population cap. Linux reported 8 MiB including its
buffer-accounting factor. No sysctl or host-wide networking setting was changed.
The harness captures owned-socket UDP drops independently of application metrics.

| 100,000-session case | OS-default buffers | Requested 4 MiB buffers |
| --- | ---: | ---: |
| Verified delivered messages | 400,000 | 400,000 |
| Measured phase seconds | 7.657257 | 4.012819 |
| Achieved deliveries/s | 52,238 | 99,681 |
| RTT p99 bound | 4,294.967296 ms | 1.048576 ms |
| Maximum RTT | 7,281.622028 ms | 3.354113 ms |
| New server receive drops during phase | 468,617 | 0 |
| New client receive drops during phase | 235,750 | 0 |

The buffered run passed every delivery, per-user, departure, counter and cleanup
check. Setup was 11.29 seconds and retirement 2.46 seconds (exact receipts retain
the measured values). Server RSS was 4,855.17 MiB at the measured gate. This is a
substantial retained-memory cost compared with the former plaintext WebSocket
implementation, and performance parity has not been established across metrics.
The initial population-wide proof burst still incurred receive drops before the
timed phase; the zero-drop statement applies specifically to measurement.

The first maintained throughput screen used identical clients and payload checks,
three alternating-order runs per transport/case, four workers, disjoint CPU groups,
1 KiB values, 32 private pairs, 1,024 fanout readers and a four-message fanout window.
All 18 runs passed their correctness and exact counter checks. Values are medians.

| Reliable workload | Plaintext WebSocket deliveries/s | WebTransport stream deliveries/s |
| --- | ---: | ---: |
| Private only | 1,288,462 | 627,196 |
| Room fanout only | 1,671,963 | 918,117 |
| Mixed, combined private + fanout | 1,587,788 | 762,742 |
| Mixed, private component | 781,693 | 92,503 |
| Mixed, fanout component | 806,284 | 670,722 |

These results contradict a general equal-throughput claim. They also compare
plaintext TCP with encrypted QUIC, so an encrypted control and controlled-loss/
freshness tests remain required. Reliable message semantics were retained rather
than changing this workload to discard messages to produce a higher rate.

`docs/reports/webtransport-buffered-2026-10-21.json` contains each sample, medians,
CPU/RSS/latency, capacity results, source and frozen-binary hashes. Complete receipts
and exact commands are under `artifacts/webtransport/`. The source/binary association
is the explicit frozen `buffered-direct-*` build, not any later optimization.

## Parity work after the explicit acceptance decision

The user requires continued pursuit of throughput and memory parity before
accepting this pivot. Datagram freshness alone does not satisfy that requirement.
The current candidate remains **unaccepted**.

Two measured changes are retained. Input now yields after the same 256-message
fairness quantum used by the native listener, instead of yielding after every
message; large fanouts retain their independent recipient-work quantum. In two
alternating runs per case, that change increased private throughput from about
642k to 1,393k deliveries/s, room fanout from 894k to 1,037k, and mixed combined
traffic from 764k to 1,065k. Exact content, FIFO, counters and cleanup passed in all
12 runs. These are four-second local workloads, not production capacity claims.

The output queue now keeps one packet inline and allocates overflow only when
needed, rather than retaining Tokio mpsc's initial 32-packet block for every peer.
Producers still serialize observer gates and FIFO insertion under the same lock.
Closing refuses later output, drains already accepted output before the genuine
close ACK, and dropping the receiver releases every queued reservation. Ready
output avoids wakeup registration; empty-to-nonempty transitions wake the receiver.
Overflow allocation is exposed through the output-capacity gauge. Three focused
tests exercise capacity/FIFO/drain, receive cancellation/concurrent wakeups, and
reservation release on receiver drop. No stream limits or application message
limits changed.

The final comparison uses three rotated fresh-process runs per case and mode,
four pinned relay workers on CPUs 0–3, eight pinned client workers on CPUs 8–15,
1 KiB values, 32 private pairs, and 1,024 room readers with fanout window four.
The TLS WebSocket fixture includes a TLS 1.3 frontend plus a loopback TCP hop;
both processes share CPUs 0–3 and both CPU/RSS totals are included. Its extra
hop, tasks and buffering prevent treating it as an intrinsic native WSS ceiling.
The earlier TLS screen allowed the frontend CPUs 0–7 and is kept separately;
those numbers must not be merged into these medians. All 27 final runs passed.

| Reliable workload | Plain WebSocket deliveries/s | TLS WebSocket fixture deliveries/s | Current WebTransport deliveries/s |
| --- | ---: | ---: | ---: |
| Private only | 1,293,729 | 1,054,499 | 1,382,190 |
| Room fanout only | 1,648,225 | 936,767 | 1,065,243 |
| Mixed, combined | 1,537,475 | 987,395 | 1,104,088 |
| Mixed, private component | 803,874 | 421,760 | 295,758 |
| Mixed, fanout component | 733,600 | 565,634 | 808,023 |

Combined medians are calculated per run before aggregation; they need not equal
the sum of the two component medians. WebTransport's private-only rate is 6.8%
above plain WebSocket, but WebTransport room fanout remains 35.4% below plain
WebSocket, and mixed combined throughput is 28.2% below plain WebSocket. Mixed private traffic is also 29.9% below the encrypted fixture while
fanout competes. Aggregating the lanes would hide this remaining fairness gap.

| Workload | Plain WS CPU cores / RSS MiB | TLS fixture CPU cores / RSS MiB | WebTransport CPU cores / RSS MiB |
| --- | ---: | ---: | ---: |
| Private only | 2.11 / 7.09 | 2.40 / 14.23 | 3.52 / 15.16 |
| Room fanout only | 1.66 / 8.42 | 1.98 / 39.30 | 3.30 / 75.59 |
| Mixed | 2.06 / 8.40 | 2.34 / 41.61 | 3.43 / 81.01 |

Higher throughput in the encrypted fixture comparison does not establish lower
CPU cost. RSS is measured at phase gates and excludes kernel socket memory; it
is neither continuous peak memory nor requested live heap. The machine-readable
report includes every sample, ranges, client resources, both lane latency
distributions, source hashes and frozen executable hashes.

The current 100,000-session run and a fresh plaintext control used the same
candidate executable, maintained verifier, population, payload and offered rate.
Each completed all 400,000 independently checked deliveries, plus population-wide
pre/post routing proofs and exact departure/closure checks.

| 100k metric | Plain WebSocket | Current WebTransport |
| --- | ---: | ---: |
| Offered rate | 100,000/s | 100,000/s |
| Achieved rate | 99,789.6/s | 99,636.6/s |
| Timed elapsed | 4.0084 s | 4.0146 s |
| RTT p50 upper bound | 0.262 ms | 0.524 ms |
| RTT p99 upper bound | 0.524 ms | 1.049 ms |
| Maximum observed RTT | 2.761 ms | 11.934 ms |
| Server CPU cores over timed work | 0.486 | 2.160 |
| Server RSS at measured gate | 112.83 MiB | 4,653.48 MiB |
| Population setup | 0.678 s | 11.511 s |
| Population retirement | 0.625 s | 2.393 s |

WebTransport RSS fell 201.69 MiB (4.15%) from the buffered direct-stack checkpoint's
4,855.17 MiB. It is still about 41 times the plaintext server RSS. Neither this
improvement nor meeting the offered message count establishes memory parity.
The timed phase added no server/client UDP receive drops; the earlier population
proof burst did incur drops. Final sessions, physical QUIC connections, queued
payload, read reservations, variables and output metadata all reached zero.

Diagnostic gperftools profiles with 8,194 sessions show about 390 MiB of requested
live allocations before the queue change and 374 MiB in the queue experiment.
These replacement-allocator runs are excluded from ordinary timings and RSS
comparisons. Frame pointers identify approximately 62% of retained bytes under
QUIC accept/setup, including about 28% directly in QUIC connection state and 19%
in stream maps. Quinn's default remote stream credits prepopulate maps for 100
bidirectional and 100 unidirectional streams. HTTP/3, crypto and relay task state
add further cost. These are measured targets for further work, not proof that
removing any one structure can close the gap.

The new freshness verifier generates 360 independently stamped 1 KiB states at
60 Hz alongside a reliable 1 MiB bulk value. Probes include the initial period
before the first state arrives; old datagrams cannot replace a newer sequence.
Netem affects only the external transport port in a temporary namespace, in both
directions. It uses matching impairment parameters and seeds, not identical
packet-loss patterns across protocols. The internal TLS proxy hop is unimpaired.

All nine final-candidate clean/loss/reordering checks passed. The clean runs
received every state with p99 freshest-state age around 17 ms. With 20 ms
one-way delay and 2% packet loss, the candidate datagram run had p99 state age
79 ms and 12/360 states unreceived; reliable WebTransport was 622 ms and the TLS
WebSocket fixture 1,061 ms, with all states received. The datagram run's bulk
value arrived after 1.905 s versus 0.665 s for the reliable-stream run and 1.128 s
for TLS WebSocket. With added jitter/reordering, 36 datagrams arrived out of order,
the newest-state check held, p99 state age was 127 ms, and 5/360 states were
unreceived. Its bulk value took 7.321 s. These one-run-per-profile candidate checks
show correctness and the freshness/bulk tradeoff; they do not establish a general
loss-performance ranking. The prior buffered-stack screen has three clean/loss
repetitions per mode and is reported separately. Missing datagrams mean unreceived
within the observation window, not a count attributed solely to network loss.

Validation: 264 Rust tests passed, one ignored; all-target Clippy with warnings
denied passed; 70 JS tests, TypeScript and the browser bundle build passed.
Chromium 155 exercised the actual bundle, including datagrams, typed values,
cross-room delivery, 128 KiB and 5 MiB payloads, concurrent variables, departure
and graceful close; all seven final connection/storage gauges were zero.
The first browser launch lost its fixture when its launcher exited; that negative
receipt is retained separately, and the live-fixture rerun passed.

Full per-sample results and provenance are in
`docs/reports/webtransport-parity-2026-10-21.json`; raw receipts and exact commands
are under `artifacts/webtransport/`. The retained candidate executable is
`artifacts/webtransport/binaries/parity-queue-relay`. Test-only TLS support is in
`examples/tls_front.rs`; `scripts/freshness-matrix.py --help` documents isolated
impairment runs. Build the maintained tools with:

```sh
cargo build --locked --release --bin war-relay --example inspection --example capacity --example tls_front --example state_freshness
```

Further acceptance work remains: mixed-traffic fairness and retained connection
memory, observer cancellation/re-subscribe checks, the full concurrent 100k/8192
inspection workload, broader overload/shutdown acceptance, QUIC buffer accounting,
and final packaging/deployment documentation. The datagram benefit is established
for the exercised freshness workloads; the requested overall parity is not.

## Compact unused QUIC stream credits

Quinn's eager map entry for every unused remote stream credit was the next
measured memory target. Lowering credits would change the transport allowance;
this change preserves the logical map with compact ranges and creates an entry
when mutable stream state is needed. Closed and unknown streams remain absent;
a valid unopened stream still reads as an empty slot. Active lookups check
initialized state first. Parameter updates visit initialized send state without
materializing empty placeholders. Advertised credits, flow-control windows,
crypto, congestion control, validation and the application wire contract remain
unchanged.

The patch changes three files in published `quinn-proto` 0.11.19. Its archive
checksum, upstream Git commit and complete patch are recorded in
`vendor/provenance.json` and `vendor/quinn-sparse-streams.patch`. The vendor checker
now supports each package's declared patch and verifies all 121 vendored files
against three original archives and two patches. Other package files and licenses
are unchanged.

The dependency passed 315 protocol/map tests in a standalone copy whose three
modified files match the vendored candidate byte for byte. A copy avoids mutating
the published lockfile in the vendor tree. Added tests compare 20,000 mixed map
operations with an independent standard hash map and confirm that 2,000 unused
credits need no hash slots. The relay's 264 Rust tests passed with the candidate,
one was ignored, and all-target Clippy with warnings denied passed. Chromium 155
passed all nine delivered-bundle checks, including datagrams, a 5 MiB value,
variables, departure and graceful close. All seven final connection/storage
gauges were zero.

Three alternating runs per case compared `parity-queue` with `quinn-sparse-active`,
using the same frozen client, four pinned server workers on CPUs 0–3, eight client
workers on CPUs 8–15, 1 KiB values, 32 private pairs and 1,024 room readers with
fanout window four. All 18 runs passed content, FIFO, counter and cleanup checks.

| Reliable workload | Prior candidate deliveries/s | Compact-credit candidate deliveries/s |
| --- | ---: | ---: |
| Private only | 1,410,836 | 1,399,870 |
| Room fanout only | 1,079,549 | 1,068,377 |
| Mixed, combined | 1,112,477 | 1,112,644 |
| Mixed, private component | 299,171 | 296,449 |
| Mixed, fanout component | 813,306 | 816,196 |

Private and room medians are about 1% lower in this screen; mixed throughput is
effectively unchanged. This is a memory improvement without a throughput-gain
claim. Room-fanout RSS fell from 75.48 to 67.75 MiB; mixed RSS fell from 80.79 to
72.65 MiB. The report retains all samples, ranges, CPU and lane latency metrics.

| 100k metric | Prior candidate | Compact-credit candidate |
| --- | ---: | ---: |
| Verified timed deliveries | 400,000 | 400,000 |
| Timed elapsed | 4.0146 s | 4.0125 s |
| Achieved delivery rate | 99,636.6/s | 99,687.3/s |
| RTT p99 upper bound | 1.049 ms | 1.049 ms |
| Maximum observed RTT | 11.934 ms | 3.451 ms |
| Server CPU over measured interval | 2.160 cores | 2.131 cores |
| Server RSS at measured gate | 4,653.48 MiB | 3,876.73 MiB |
| Population setup | 11.511 s | 11.278 s |
| Population retirement | 2.393 s | 2.343 s |

The population run saves 776.75 MiB, about 16.7%. All users pass pre/post routing
proofs and exact departure/close checks. Timed server/client UDP receive drops
remain zero; earlier proof bursts still incur drops. Application and independent
QUIC connection counts, queued payload, read reservations, variables and output
metadata finish at zero. That does not mean RSS returned to the OS: the candidate
still reports 3,903.33 MiB at the post-retirement gate. Logical counters, requested
live heap and process RSS are separate measurements.

An 8,194-session gperftools experiment reduced requested live bytes from
392,344,586 to 325,430,253, about 17.1%. Replacement-allocator timings/RSS are
excluded from ordinary comparisons. The final revision changes active lookup
without changing the representation and is measured separately above. Large
connection, event-delivery and application-task allocations remain targets.

All nine clean/loss/reordering candidate checks passed. Under 20 ms one-way delay
and 2% loss, datagrams received 347/360 states and had p99 newest-state age 73 ms;
reliable streams received all states, with p99 ages 661 ms for WebTransport and
1,061 ms for the TLS WebSocket fixture. With added jitter/reordering, 21 datagrams
arrived out of order, the newest-state check held, 355 states arrived and p99 age
was 120 ms. These single candidate checks are not a statistical protocol ranking;
the report also records reliable bulk completion and the freshness/bulk tradeoff.

The exact executable is `artifacts/webtransport/binaries/quinn-sparse-active-relay`.
`docs/reports/webtransport-sparse-streams-2026-10-21.json` contains the samples,
hashes and validation scopes. Earlier trials and the initial range-first lookup
variant remain separately recorded. Prior plain/encrypted comparisons are not
redefined by this memory experiment.

Acceptance is still unproven: 3,876.73 MiB remains roughly 34 times the measured
plaintext server RSS, and the room/mixed throughput and mixed private-lane gaps
remain. Continue with event/task storage and reliable scheduling; the previously
listed observer, full-load, buffer-accounting, lifecycle and packaging work also
remains outstanding.

## Lazy per-connection event storage

The next profile target was the runtime's per-connection event channel. Published
`quinn` 0.11.12 uses a Tokio unbounded mpsc channel whose first block holds 32 large
events on this 64-bit machine. The local channel keeps its first event inline and
allocates overflow only when needed. There is one endpoint-owned sender and one
connection-owned receiver; endpoint serialization, FIFO, close/rebind events,
pending-reader wakeups and receiver retirement retain their contracts. Receiving
still consumes Tokio's cooperative budget. The public API used for that requires
Tokio 1.47 and `rt`, now declared in the patched manifest; the product already
uses Tokio 1.53.3 with `rt`.

An initial burst check found that a drained 10,000-event burst retained 16,384
overflow slots. The final implementation releases oversized empty buffers while
retaining small buffers for reuse. The reuse threshold matches the original mpsc
block size (32 on 64-bit, 16 otherwise); it is not a queue-depth limit. The negative
test also exposed a second panic if cleanup encountered a poisoned queue lock.
Cold retirement now releases events safely during unwinding, and both behaviors
have regression checks. Reliable events are not discarded to raise throughput.

The complete package patch is `vendor/quinn-event-channel.patch`. Six original or
added package files are declared in `vendor/provenance.json`; the verifier checks
all 152 vendored files against four published archives and three patches. Original
licenses and other package files are preserved.

The final source passed 28 default runtime/channel tests plus all three explicitly
run receive-window stress tests. Their standalone copy matches all six changed
files byte for byte and explicitly pins the product's patched `quinn-proto`
0.11.19 and Tokio 1.53.3. Early prototype tests used the archive's older lockfile
and are kept separately, not cited as the final pinned-version evidence. The
application's 264 Rust tests passed, one was ignored, and all-target Clippy with
warnings denied passed. A legacy WebSocket fixture failure was traced to valid
global traffic preceding an authentication reply: commit `f75bcd5` waits for the
actual reply and validates interleaved data, leaving product ordering unchanged.
Three targeted repetitions and the full suite pass after that correction.

Final timing uses three alternating fresh-process runs per case with the same
frozen verifier, four pinned server workers on CPUs 0–3, eight client workers on
CPUs 8–15, 1 KiB values, 32 private pairs, 1,024 room readers and fanout window four.
All 18 runs pass content, sender, FIFO, exact counters and cleanup checks.

| Reliable workload | Prior candidate deliveries/s | Event-channel candidate deliveries/s |
| --- | ---: | ---: |
| Private only | 1,402,391 | 1,400,630 |
| Room fanout only | 1,068,535 | 1,109,825 |
| Mixed, combined | 1,110,660 | 1,153,385 |
| Mixed, private component | 295,117 | 314,225 |
| Mixed, fanout component | 814,502 | 839,160 |

Private throughput is effectively unchanged in this screen. Room and mixed
throughput improve about 3.9%. Room-fanout RSS falls from 68.00 to 55.24 MiB;
mixed RSS falls from 72.50 to 59.27 MiB. CPU, latency distributions, individual
samples and ranges are retained in the report; these short local workloads do
not establish production capacity or a latency SLA.

| 100k metric | Prior candidate | Event-channel candidate |
| --- | ---: | ---: |
| Verified timed deliveries | 400,000 | 400,000 |
| Timed elapsed | 4.0125 s | 4.0125 s |
| Achieved delivery rate | 99,687.3/s | 99,688.3/s |
| RTT p99 upper bound | 1.049 ms | 1.049 ms |
| Maximum observed RTT | 3.451 ms | 2.342 ms |
| Server CPU over measured interval | 2.131 cores | 2.054 cores |
| Server RSS at measured gate | 3,876.73 MiB | 3,293.20 MiB |
| Population setup | 11.278 s | 10.999 s |
| Population retirement | 2.343 s | 2.237 s |

The final population run saves 583.53 MiB, about 15.1%. Both complete all-user
pre/post routing proofs and exact departure/close checks. Timed UDP receive drops
are zero on client and server; earlier proof bursts still incur drops. All seven
checked final connection/storage gauges reach zero. RSS excludes kernel socket
memory and is measured at gates, not as continuous peak or requested live heap.

The replacement-allocator 8,194-session diagnostic reduces requested live bytes
from 325,430,253 to 275,587,957, about 15.3%. After logical gauges reach zero,
requested live bytes continue falling to 9,731,291 at one second and remain there
at five seconds, while instrumented process RSS remains much higher. That is
evidence to distinguish asynchronous cleanup and allocator retention from active
connection storage. These allocator-replaced timings/RSS are excluded from
ordinary medians and are not generalized to production allocator reclamation.

All nine final clean/loss/reordering regression checks pass. Under 20 ms one-way
delay and 2% loss, datagrams receive 347/360 states with p99 newest-state age 73 ms;
reliable WebTransport receives all states with p99 age 661 ms, and the TLS fixture
receives all states with p99 age 1,181 ms. Reordering still leaves the newest-state
check intact. These are single candidate samples per profile; full bulk timing,
losses, reorder counts, freshness and generation-lag metrics are reported without
claiming a statistical protocol ranking. Chromium 155 passes all nine delivered
bundle checks, including typed values, datagrams, a 5 MiB value, variables,
departure and genuine graceful close, with seven final gauges zero.

The retained executable is
`artifacts/webtransport/binaries/quinn-event-reclaim-relay`.
`docs/reports/webtransport-event-channel-2026-10-21.json` contains the final
measurements, earlier trials, provenance hashes and validation scopes. The
3,293.20 MiB RSS remains roughly 29 times the last measured plaintext control;
room/mixed throughput and mixed private-lane parity are still unmet. Continue
with active task/connection storage and reliable scheduling, plus the previously
listed acceptance work. This checkpoint does not accept or release the pivot.

## Shared session ownership and shorter-lived admission storage

Compiler layout and the replacement-allocator profile identified the application
connection task as the next memory target: 4,736 bytes per task at the previous
checkpoint. `web-transport-quinn::Session` is a 776-byte handle containing HTTP
metadata and per-clone polling state. Moving and cloning it through nested async
functions reserved multiple copies of that inline storage for each live session.
The adapter uses its shared async APIs, which own their waiter registrations,
rather than the per-clone `poll_*` operations.

The application task and output sink now share one `Arc<Session>`. The larger
admission future is allocated separately before spawning and released when the
handshake completes; its result is already Arc-owned. This reduces the persistent
Tokio task cell from 4,736 to 2,048 bytes, the inner session future from 3,304 to
1,768 bytes, and the output sink from 976 to 208 bytes. These are compiler layout
measurements, not whole-process totals. One persistent Arc allocation and one
transient admission allocation per connection replace the copied handles and
larger task storage. There is no new per-message allocation. The admission lease
is captured before spawning so that cancellation before the task's first poll
also releases its connection count. Wire behavior, routing, transport parameters,
queue limits, timeouts, crypto and congestion control are unchanged.

The ordinary release build is separately frozen as
`artifacts/webtransport/binaries/shared-session-relay`, SHA256
`5f5e91e06fe06bb265d26082fdb076f345e02c160971fc6a20c4a1cd0fbcdc40`.
The build manifest records the source diff and hashes. Diagnostic `-Z` compiler
flags were not used for the timed executable. Three alternating fresh-process
runs per reliable case use the same frozen client and previous server executable,
four pinned server workers, eight client workers, 1 KiB values, 32 private pairs,
1,024 fanout readers and window four. All 18 runs pass full payload, sender, FIFO,
counter, close and cleanup checks.

| Reliable workload | Previous candidate deliveries/s | Shared-session candidate deliveries/s |
| --- | ---: | ---: |
| Private only | 1,398,619 | 1,396,457 |
| Room fanout only | 1,080,688 | 1,099,968 |
| Mixed, combined | 1,159,945 | 1,160,194 |
| Mixed, private component | 316,671 | 313,677 |
| Mixed, fanout component | 843,273 | 846,516 |

The useful result is lower storage with essentially unchanged private/mixed
throughput. Room fanout is 1.8% higher in this local sample. Fanout RSS decreases
from 55.23 to 52.58 MiB, mixed RSS from 59.18 to 56.04 MiB. Individual repetitions,
latency bounds, CPU, client RSS and ranges remain in the report; no production
latency or general speedup claim follows from these short loopback screens.

The 100k population comparison was expanded to three fresh runs per executable
in alternating order after an initial candidate sample showed a higher maximum
RTT. Similar spikes appeared in the control. All six runs complete 400,000 timed
deliveries plus independent all-user pre/post routing proofs, exact departures
and genuine close acknowledgements. All seven final connection/storage gauges
reach zero. Timed client/server UDP receive drops are zero; the untimed proof
bursts can still drop UDP packets. The first Python wrapper incorrectly treated
`capacity.run()`'s `None` return as a result object after it wrote its successful
receipt. Both child exit codes and that receipt passed; the wrapper was corrected,
and its post-validation exception is preserved in the comparison manifest.

| 100k metric (median of three runs unless noted) | Previous candidate | Shared-session candidate |
| --- | ---: | ---: |
| Verified timed deliveries in each run | 400,000 | 400,000 |
| Timed elapsed | 4.0139 s | 4.0134 s |
| Achieved delivery rate | 99,653.4/s | 99,667.0/s |
| RTT p50 upper bound | 0.524 ms | 0.524 ms |
| RTT p99 upper bound range | 1.049–2.097 ms | 1.049 ms |
| Maximum RTT across all three runs | 11.684 ms | 11.330 ms |
| Scheduling-lag p99 upper bound in each run | 2.097 ms | 2.097 ms |
| Server CPU over measured interval | 2.093 cores | 2.073 cores |
| Server RSS at measured gate | 3,295.63 MiB | 3,050.17 MiB |
| Server RSS range | 3,295.07–3,296.31 MiB | 3,049.38–3,050.62 MiB |
| Server RSS after retirement | 3,299.76 MiB | 3,053.47 MiB |
| Population setup | 11.375 s | 11.375 s |
| Population retirement | 2.227 s | 2.334 s |

Median measured RSS decreases by 245.46 MiB (7.45%). RSS excludes kernel socket
memory and is sampled at phase gates. The separate 8,194-session replacement-
allocator diagnostic corroborates lower active storage: requested live bytes at
ready decrease from 275,587,957 to 253,317,298 (8.08%). After logical cleanup, the
new run settles to 9,753,018 requested live bytes at one second and remains there
at five seconds. Instrumented allocator RSS is excluded from ordinary comparisons.
This does not claim that production process RSS immediately returns to the OS.

All nine final clean/loss/reorder checks pass, including full bulk/payload checks,
stale-state suppression and zero final gauges. Under 20 ms one-way delay and 2%
loss, the datagram sample receives 345/360 states with p99 newest-state age 73 ms;
the reliable WebTransport sample receives 360/360 with p99 age 1,721 ms, and the
TLS fixture receives 360/360 with p99 age 981 ms. With jitter/reordering added,
datagram p99 newest-state age is 109 ms with 358/360 updates, while reliable
WebTransport is 5,941 ms and TLS 741 ms. Bulk completion in those reordering
samples is respectively 7,167, 6,137 and 804 ms. The datagram freshness benefit
does not establish better reliable bulk progress. Each is one candidate sample;
matching seeds and impairment parameters do not impose identical loss trajectories.
Missing updates are unreceived observations, not all attributed to network loss.

Chromium 155 passes all nine actual delivered-bundle checks: three sessions,
typed broadcast, cross-room private delivery, datagrams, 128 KiB and 5 MiB values,
concurrent variables, unsafe-number rejection, departure and real graceful close.
All seven final gauges are zero; the screenshot is
`output/playwright/webtransport-shared-session.png`. The product's 264 Rust tests
pass with one ignored, Clippy with warnings denied passes, and all 152 vendored
files verify against their declared archives/patches. No dependency source was
changed at this checkpoint. Browser, HTTP, benchmark and network-namespace fixtures
were stopped and removed after verification.

`docs/reports/webtransport-shared-session-2026-10-21.json` retains individual
samples, ranges, complete freshness/bulk/generation-lag measurements, source and
binary provenance, diagnostics, and validation scopes. The 3,050.17 MiB median
RSS is still approximately 27 times the earlier 112.83 MiB plaintext control.
Room/mixed throughput and mixed private-lane parity remain unmet. Continue active
QUIC/HTTP3 connection storage and reliable scheduling work, plus the previously
listed acceptance work. This measured memory improvement does not accept or
release the pivot.

## Shared-body chunk writes on the direct engine

The refreshed current-executable profile identifies repeated stream writes,
QUIC packet construction, buffer ownership and crypto as substantial costs.
`write_all` contributes 4.85% of private and 4.30% of mixed self samples in separate
8-second, 499 Hz user-cycle captures. These are sampled costs, not a promised
speedup. The earlier rejected chunk-write experiment used the old driver and a
32-client private probe; it did not cover the current direct engine's larger
fanout/mixed workloads.

The reliable writer now combines its four-byte record length and small inline
CBOR prefix into one owned header, then submits that and a shared immutable body
through the direct stream's `write_all_chunks` API. QUIC owns the chunks through
retransmission instead of copying the payload for each recipient. This still
allocates a small header, increments shared ownership, and incurs packet/crypto
copies; it is not an end-to-end zero-copy or zero-allocation claim. Framing, FIFO,
routing, reservation lifetime, per-record deadlines, transport parameters and
crypto remain unchanged. Empty graceful-close ACK records retain their format.

The final release executable is byte-identical to the timed candidate, SHA256
`8feaec3e5986c0c72299d38d106543579587d47587e3ba7e17f6b6728c1a2b7d`,
retained as both `binaries/direct-chunks-relay` and
`binaries/direct-chunks-final-relay` under `artifacts/webtransport/`. The manifest
checks final product source hashes and rebuilt executable equality. Only a real
write-pressure fixture/test was added after the throughput measurements.

All 18 main comparison runs pass full payload, sender, FIFO, counter, lifecycle
and cleanup checks. Three alternating fresh processes per case/variant use the
same frozen client, four pinned server workers on CPUs 0–3 and eight client
workers on CPUs 8–15, four seconds, 1 KiB values, 32 private pairs, 1,024 fanout
readers and fanout window four.

| Reliable workload | Previous deliveries/s | Chunk-write deliveries/s | Change |
| --- | ---: | ---: | ---: |
| Private only | 1,404,079 | 1,481,922 | +5.5% |
| Room fanout only | 1,119,984 | 1,315,752 | +17.5% |
| Mixed, combined | 1,163,520 | 1,362,269 | +17.1% |
| Mixed, private component | 314,795 | 356,607 | +13.3% |
| Mixed, fanout component | 850,140 | 1,006,568 | +18.4% |

Component and combined medians are computed independently. Server CPU medians
are 3.515 → 3.497 cores private, 3.259 → 3.155 fanout and 3.392 → 3.354 mixed.
Server RSS medians are 13.81 → 13.34 MiB private, 52.50 → 48.31 MiB fanout and
56.32 → 53.45 MiB mixed. These are whole-workload observations at differing
completed-work rates, not isolated CPU cost per message. The candidate's p99
upper bounds remain 1.049 ms private-only, 4.194 ms fanout-only, and respectively
4.194/8.389 ms for mixed private/fanout. All individual samples, ranges, client
CPU/RSS and observed maxima are retained in the report.

Sharing can extend a backing allocation's lifetime, so large records were checked
separately with another 12 passing alternating runs. These use the same four-
second workload and frozen verifier, 32 private pairs at 64 KiB and four pairs at
1 MiB. All payload bytes and lifecycle/resource counts are checked.

| Private content size | Previous deliveries/s | Chunk-write deliveries/s | Previous RSS | Chunk-write RSS |
| --- | ---: | ---: | ---: | ---: |
| 64 KiB | 40,123 | 40,650 | 36.12 MiB | 40.16 MiB |
| 1 MiB | 2,177 | 2,156 | 95.08 MiB | 52.23 MiB |

Large-record throughput is effectively unchanged in this screen. The 64 KiB RSS
increase is a tradeoff requiring follow-up, not a memory win. Its samples are
33.63–36.36 versus 36.08–42.94 MiB. The 1 MiB samples are 88.60–98.17 versus
32.99–58.66 MiB; the larger payload is retained once while transmission proceeds.
Both sizes retain a 33.55 ms p99 upper bound. The read assembler's geometric
capacity and shared lifetime are next targets; do not infer universal memory
savings from the small-message fanout result.

All six fresh 100k runs pass, with 400,000 verified timed deliveries each,
independent physical-connection counts, all-user pre/post routing proofs, exact
close/departure checks, zero timed UDP receive drops on both endpoints and seven
final gauges zero. Median results are:

| 100k metric | Previous candidate | Chunk-write candidate |
| --- | ---: | ---: |
| Timed elapsed | 4.0141 s | 4.0136 s |
| Achieved deliveries/s | 99,648.6 | 99,660.8 |
| RTT p50 / p99 upper bound in every run | 0.524 / 1.049 ms | 0.524 / 1.049 ms |
| Maximum observed RTT across three runs | 12.309 ms | 10.937 ms |
| Server CPU | 2.014 cores | 2.051 cores |
| Server RSS at measured gate | 3,049.25 MiB | 3,048.38 MiB |
| Server RSS after retirement | 3,054.08 MiB | 3,052.57 MiB |
| Setup | 11.399 s | 11.577 s |
| Retirement | 2.244 s | 2.200 s |

Capacity RSS and latency are essentially unchanged. The small capacity CPU
increase is reported rather than claiming a CPU improvement across all workloads.
RSS excludes kernel socket memory and is gate-sampled, not a continuous peak.
The 8,194-session replacement-allocator ready snapshots also remain essentially
unchanged at 253.32 MB requested live bytes. The new run settles to 9,751,260
requested live bytes after one second and remains there at five seconds; these
instrumented results are excluded from ordinary timing/RSS comparisons.

A new real-transport pressure test advertises 64 KiB client stream receive credit
and leaves a 12 MiB delivery unread, exceeding the default 10 MB QUIC send window.
The writer must stay pending, then hit its configured one-second deadline. The
test verifies exactly one timeout, release of application queued/read-budget
credit, continued private routing in another room before and after the timeout,
and zero connection/output metadata credit after shutdown. The focused run and
full suite pass. This covers one concrete partial-write cancellation path;
broader overload, fanout cancellation and shutdown/retirement fallback acceptance
remain required.

All nine clean/loss/reorder regression samples pass. With 20 ms one-way delay and
2% loss, datagrams receive 348/360 updates with p99 newest-state age 71 ms; reliable
WebTransport receives 360/360 at 1,102 ms and the TLS fixture 360/360 at 981 ms.
Adding jitter/reordering gives datagrams 354/360 at 117 ms, reliable WebTransport
360/360 at 5,342 ms and TLS 360/360 at 741 ms. Reliable 1 MiB bulk completion in
those reordering samples is 7,239, 5,394 and 810 ms respectively. Freshness and
bulk progress remain separate outcomes. One sample per mode is regression
evidence, not a statistical ranking; matching impairment parameters/seeds are
not identical loss trajectories, and missing updates are unreceived observations.

The current bundle passes all nine Chromium 155 checks, including typed values,
datagrams, a 5 MiB message, concurrent variables, departure and real graceful
close; seven final gauges are zero. Screenshot:
`output/playwright/webtransport-direct-chunks.png`. The product has 265 passing
Rust tests, one ignored; all-target Clippy with warnings denied and the 152-file
vendor verifier pass. Browser/HTTP/relay fixtures and impairment namespaces were
stopped and removed. No dependency source changed.

A new candidate profile confirms that crypto and packet construction lead the
visible self samples; it is retained alongside the baseline captures. The full
record is `docs/reports/webtransport-direct-chunks-2026-10-21.json`, with all
samples, ranges, large-payload tradeoffs, full freshness/bulk/generation-lag
metrics, provenance and validation scopes. Reliable fanout/mixed throughput is
closer to, but still below, the earlier plaintext control. Mixed private traffic
still trails the earlier encrypted fixture, and 100k server RSS remains about
27 times the plaintext control. Continue receive framing/retained capacity,
active QUIC/HTTP3 state and reliable scheduling work. This checkpoint does not
accept or release the pivot.

## Bound record growth by its announced size

The reliable reader's geometric `BytesMut` growth can retain the next power of
two even when that exceeds the complete record size. This is especially visible
just above a payload-size boundary, after CBOR envelope bytes are included. An
independent capacity diagnostic reproduces the actual old/new growth loops:

| Announced record bytes | Previous requested capacity | Capped requested capacity |
| --- | ---: | ---: |
| 0 | 0 | 0 |
| 1,038 | 1,038 | 1,038 |
| 65,550 | 131,072 | 65,550 |
| 1,048,590 | 2,097,152 | 1,048,590 |
| 5,242,894 | 8,388,608 | 5,242,894 |

The reader now assembles into a `Vec`, explicitly growing geometrically up to the
announced size with `reserve_exact`, then transferring immutable ownership into
`Bytes`. It still admits the full existing two-times-size read lease before
allocation, retaining headroom for transient reallocations. The initial/staged
read size remains 4,096 bytes; a large announcement still does not allocate the
whole body before data arrives. No message limit, byte budget, framing, delivery,
FIFO, deadline or transport/crypto setting changes. This still allocates and
copies input; it is not a borrowed-chunk decoder. Requested capacity is separate
from allocator rounding, live heap and whole-process RSS.

The ordinary release executable is
`artifacts/webtransport/binaries/capped-record-relay`, SHA256
`3a8e08cd36d9a064f1a42371cb5dcd3c5f42e967ba20df6fd55a3448dc0818fd`.
Its build manifest records product/test source hashes and the complete source
diff from `da80024`. The same frozen client verifies all comparison variants.
No benchmark process runs concurrently with builds, tests or browser fixtures.

All 12 large-payload runs pass full content, sender, FIFO, counter, close and
cleanup checks. They alternate fresh control/candidate processes three times per
size, four seconds, four pinned server workers, eight client workers, with 32
private pairs at 64 KiB and four pairs at 1 MiB:

| Private payload | Previous deliveries/s | Capped deliveries/s | Previous RSS | Capped RSS |
| --- | ---: | ---: | ---: | ---: |
| 64 KiB | 40,619 | 41,379 | 33.59 MiB | 33.48 MiB |
| 1 MiB | 2,195 | 2,228 | 60.71 MiB | 41.41 MiB |

The 1 MiB gate-sampled RSS reduction is 19.30 MiB (31.8%); samples are
58.12–69.37 versus 36.60–43.06 MiB. The 64 KiB RSS screen is effectively unchanged,
so do not attribute the prior checkpoint's entire 64 KiB variation to this one
capacity effect. Both sizes retain a 33.55 ms p99 upper bound. CPU and throughput
are effectively unchanged in these short local samples; the retained improvement
is lower large-record capacity and measured working memory, not a general speedup.

All 18 main 1 KiB runs also pass. Placement, duration and workload remain the same
as the previous checkpoint: CPUs 0–3 for four server workers, CPUs 8–15 for eight
client workers, 32 private pairs, 1,024 fanout readers and window four.

| Reliable workload | Previous deliveries/s | Capped deliveries/s | Capped CPU | Capped RSS |
| --- | ---: | ---: | ---: | ---: |
| Private only | 1,482,191 | 1,483,036 | 3.497 cores | 13.67 MiB |
| Room fanout only | 1,312,314 | 1,326,331 | 3.170 cores | 48.28 MiB |
| Mixed, combined | 1,358,356 | 1,362,212 | 3.354 cores | 53.12 MiB |

These small differences do not establish a meaningful throughput change. Current
mixed component medians are 356,840 private and 1,002,073 fanout deliveries/s,
computed independently from the combined median. P99 upper bounds remain
1.049 ms private-only, 4.194 ms fanout-only and 4.194/8.389 ms mixed private/fanout.

All six fresh 100k comparisons pass: 100,000 independently counted physical QUIC
connections, 400,000 verified timed deliveries, all-user pre/post routing proofs,
exact close/departure checks and seven final gauges zero. Timed client/server UDP
receive drops are zero; untimed proof bursts can still incur drops.

| 100k metric (median except noted) | Previous candidate | Capped candidate |
| --- | ---: | ---: |
| Timed elapsed | 4.0133 s | 4.0135 s |
| Achieved deliveries/s | 99,668.8 | 99,663.5 |
| RTT p50 / p99 upper bound in each run | 0.524 / 1.049 ms | 0.524 / 1.049 ms |
| Maximum RTT across three runs | 10.701 ms | 11.604 ms |
| Server CPU | 2.078 cores | 2.048 cores |
| Server RSS at measured gate | 3,049.06 MiB | 3,050.28 MiB |
| Server RSS after retirement | 3,053.20 MiB | 3,054.11 MiB |
| Population setup | 11.370 s | 11.505 s |
| Population retirement | 2.189 s | 2.201 s |

Capacity memory and performance are effectively unchanged; this is not a
population-memory win. RSS excludes kernel socket memory, is sampled at phase
gates and is not continuous peak or requested live heap. Individual samples,
client CPU/RSS, latency bounds/maxima and ranges remain in the report.

All nine clean/loss/reordering checks pass. Under 20 ms one-way delay and 2% loss,
the datagram sample receives 340/360 states with p99 newest-state age 114 ms;
reliable WebTransport receives 360/360 at 601 ms and the TLS fixture 360/360 at
1,061 ms. With jitter/reordering added, datagrams receive 356/360 at 120 ms,
reliable WebTransport 360/360 at 5,941 ms and TLS 360/360 at 501 ms. Reliable 1 MiB
bulk completion in those reordering samples is 7,997, 6,351 and 569 ms respectively.
The datagram freshness benefit remains a loss tradeoff and does not establish
better reliable bulk progress. One sample per mode is regression evidence, not a
statistical ranking. Matching seeds/parameters do not force identical loss
trajectories, and missing updates are unreceived observations.

The actual Chromium 155 bundle passes all nine checks, including typed values,
datagrams, a 5 MiB message, variables, departure and genuine graceful close, with
seven final gauges zero. Screenshot: `output/playwright/webtransport-capped-record.png`.
All 265 Rust tests pass with one ignored, including the existing 12 MiB blocked-
write deadline/cleanup test. All-target Clippy with warnings denied and the
152-file vendor verifier pass. No dependency source changed. Browser/HTTP/relay
fixtures and network namespaces were stopped and removed after verification.

Read-only upstream research supplies two next-step leads:
[driver scheduling overhead](https://github.com/quinn-rs/quinn/issues/914) and
[a reported packet-reordering window limitation](https://github.com/quinn-rs/quinn/issues/2710).
The pinned source independently still uses the `u128` dedup window. Neither the
reported bulk-stall cause nor the proposed scheduling improvement has been
verified for this branch. Measure a local reproducer/profile before changing
those algorithms or adding memory/complexity.

`docs/reports/webtransport-capped-record-2026-10-21.json` records all samples,
ranges, capacity diagnostics, full freshness/bulk/generation-lag data, provenance
and validation scopes. Overall throughput and memory parity remain unmet against
the earlier plaintext control; 100k RSS remains approximately 27 times higher.
Mixed private traffic also remains below the earlier encrypted fixture. Continue
active QUIC/HTTP3 storage, receive framing and reliable scheduling work, alongside
the previously listed observer, overload, combined-population, accounting and
packaging acceptance gates. This checkpoint does not accept or release the pivot.

## Causal profiling of the datagram comparison

The 2026-10-21 profiling pass separates reliable-stream results from datagram
results and identifies a benchmark-client confounder. In a controlled six-run
client swap with the server unchanged, datagram echo rate rises from 495,437 to
588,120/s (18.7%). Server/client perf recordings also identify substantial
Quinn/Tokio dispatch, scheduling and timer work; crypto accounts for about 7.3%
of server self-cycle samples with the direct client. The adapter and library
perform avoidable contiguous datagram copies, but their individual cost has not
yet been isolated. These are 32-session closed-loop diagnostics, not saturation,
fanout, browser or 100k acceptance. No product implementation changed.

See [the causal report](reports/webtransport-causal-profile-2026-10-21.md) and
[complete measurements](reports/webtransport-causal-profile-2026-10-21.json).
Throughput and memory parity remain unmet; the pivot is not accepted.

## Reuse QUIC packet-construction storage by runtime thread

The causal follow-up tested two proposed costs separately. Removing the relay's
per-peer datagram scratch buffer removes one full payload copy and one retained
mutex/Vec. Three alternating echo comparisons show essentially unchanged
throughput; this is simpler ownership, not a claimed throughput fix. The
WebTransport wrapper's separate framing allocation/copy remains.

A second experiment opportunistically processed input from the endpoint when a
connection driver was idle, then published application events and woke the normal
driver for transmit/timers. One-worker echo throughput rose from 232,429 to
246,236/s (5.9%), but four-worker throughput was effectively unchanged at
591,614 versus 592,180/s and CPU/completion rose from 4.834 to 5.009 us (3.6%).
That experiment was rejected and fully reverted. Its patch and all 12 receipts
remain in artifacts; it does not establish that a larger worker-owned redesign
would fail or succeed.

The retained Quinn change targets an independently identified storage cost:
`State::send_buffer` previously retained packet-construction high-water capacity
for every connection even after successful UDP submission. A synchronous transmit
turn now owns a `TransmitBuffer` lease, reusing one cached allocation per runtime
thread. Successful submissions return the cleared allocation for the next
connection. If socket writability becomes Pending, exact ciphertext ownership
moves back into that connection with its pending transmit metadata. The buffer
is not shared across an async wait. Nested/custom socket calls get independent
owned buffers; thread-local teardown and an already borrowed cache do not panic.
No queue-depth, flow-credit, packet-number, crypto, congestion, timer, framing,
FIFO or fairness limit changes.

The frozen ordinary release executable is
`artifacts/webtransport/binaries/shared-transmit-final-relay`, SHA256
`1a4c8830cc5e0ecf814f385a6de5023d45799d74e3beffdd69703fb6cbe9f5ad`.
The baseline `direct-datagram-relay` already contains only the scratch-copy
cleanup, so the matched comparison isolates Quinn buffer reuse. The build
manifest records the source hashes, diff and exact verifier hashes. Earlier
exploratory capacity receipts use a pre-final binary hash and are excluded from
final medians. One exploratory run also overlapped test compilation during setup;
it is preserved but excluded entirely. Final capacity repetitions 4–6 and all
ordinary throughput runs have no concurrent builds/tests/browser jobs.

All 18 WT comparisons and nine refreshed plaintext WS controls pass full content,
sender, FIFO, counters, close and cleanup checks. WT cases alternate three times
per variant; WS controls are a subsequent block, not interleaved protocol
comparisons. Four pinned server workers on CPUs 0–3, eight pinned client workers
on CPUs 8–15, four seconds, 1 KiB values, 32 private pairs, 1,024 room readers and
fanout window four:

| Reliable workload | Before deliveries/s | Reused-buffer deliveries/s | Fresh plaintext WS deliveries/s |
| --- | ---: | ---: | ---: |
| Private | 1,470,291 | 1,461,602 | 1,254,258 |
| Room only | 1,311,483 | 1,365,453 | 1,661,900 |
| Mixed aggregate | 1,352,470 | 1,400,383 | 1,487,889 |

Private throughput is effectively unchanged; room and mixed medians rise 4.1%
and 3.5% in this local screen. Those short-run gains are not a general protocol
ranking. Median server RSS falls from 13.71 to 13.09 MiB private, 48.40 to
39.00 MiB room-only, and 52.83 to 43.91 MiB mixed. Server CPU medians are
3.497, 3.134 and 3.324 cores respectively. Private p99 remains bounded by
1.049 ms, room p99 by 4.194 ms, and mixed private/fanout p99 by 4.194/8.389 ms.

Aggregate mixed traffic must not hide private-lane fairness: its new independent
private median is 356,655/s versus 791,957/s for WS, while fanout medians are
1,041,940/s versus 718,254/s. Component and aggregate medians are independently
computed and need not sum. The private lane still has a major scheduling deficit.
The fixed direct-client datagram echo comparison is unchanged at 583,121 versus
581,699/s; server CPU/completion is 4.854 versus 4.826 us. These 32-session,
one-outstanding-request diagnostics are not datagram saturation/fanout acceptance.

All nine final 100k runs pass: three before, three reused-buffer, three refreshed
WS. They use eight pinned server and eight pinned client workers, 100,000
independently counted physical connections, 400,000 timed deliveries, all-user
pre/post routing proofs and exact retirement:

| 100k metric, median | Before | Reused buffers | Fresh plaintext WS |
| --- | ---: | ---: | ---: |
| Server RSS at measured gate | 3,045.78 MiB | 2,815.90 MiB | 113.07 MiB |
| Server CPU over timed work | 2.088 cores | 2.068 cores | 0.487 cores |
| Achieved deliveries/s | 99,688.7 | 99,679.6 | 99,816.3 |
| RTT p50 upper bound | 0.524 ms | 0.524 ms | 0.262 ms |
| RTT p99 upper bound | 1.049 ms | 1.049 ms | 0.524 ms |
| Population setup | 11.383 s | 11.426 s | 0.668 s |
| Population retirement | 2.172 s | 2.155 s | 0.621 s |

The median RSS reduction is 229.88 MiB (7.55%). Reused-buffer samples are
2,814.58–2,816.82 MiB versus 3,045.40–3,047.23 MiB before. The observed reused-buffer
100k p99 upper bound is the same in all new runs, not a production tail SLA.
Timed UDP receive-drop deltas are zero; untimed population-proof bursts still
incur drops. All seven final connection/storage gauges are zero. Full receipts
separately retain TCP ownership census, UDP queues/drops and machine CPU samples;
RSS excludes kernel allocations. Complete TCP/UDP kernel allocation bytes were
not measured, so this is not total-system memory parity. Server RSS remains about
25 times the new plaintext control.

A matched diagnostic with 8,194 real ready sessions and the same replacement
allocator confirms requested live bytes decrease from 253,051,118 to 233,398,064
(19,653,054 bytes, 7.77%); live allocation counts decrease from 804,224 to 796,042.
The former approximately 22.4 MB packet-buffer growth allocation group disappears.
These are requested live allocations, not ordinary RSS or timing measurements;
net change includes other small phase-dependent allocation differences.

Two separate eight-second mixed CPU captures pass the full verifier and report
zero lost samples. Packet construction, encryption, stream reads, connection
polling, Notify and timers remain prominent; scratch reuse does not eliminate
those costs. Captures are excluded from ordinary medians.

After timing, the formatter collapsed one multiline `if` expression. The
measured source snapshot and executable remain frozen. Applying the same
formatter to that snapshot reproduces the current source exactly. The formatted
release is separately frozen as `shared-transmit-formatted-relay`, SHA256
`9e52547dc6d340761207fba4e9e860fa04c8dccfde4f7f78562a83a51098ca82`.
It independently passes all 266 tests, Clippy, a full 100k capacity run
(2,815.56 MiB, 400,000 checked deliveries), a full mixed run (1,409,380/s),
and all nine Chromium checks with final gauges zero. Those single verification
runs are not pooled into the repeated medians above. The report preserves both
build identities and the complete formatting-only diff.

Validation: 266 relay tests pass with one existing ignored test; all-target
Clippy with warnings denied passes. All 28 Quinn default runtime/channel tests
and three explicitly forced receive-window stress tests pass in a standalone
copy with the seven modified files byte-identical to the vendor tree, pinned to
product quinn-proto 0.11.19 and Tokio 1.53.3. The archive/patch check verifies
153 files against four original packages and three patches. The new real-UDP
regression forces WouldBlock followed by Pending, constructs another connection's
packet on the same runtime thread, then verifies the retained ciphertext and
both received payloads before endpoint retirement.

Actual Chromium 156 passes all nine delivered-bundle checks, including typed
values, private/broadcast/datagrams, 128 KiB and 5 MiB messages, variables, unsafe
Number rejection, departure and real graceful closure. Seven final gauges are
zero. All nine clean/loss/reorder freshness checks pass. In this one sample per
mode/profile, loss datagrams have 75.6 ms p99 newest-state age and receive 345/360
updates, versus 640.7 ms and all updates for the stream and 1,061.0 ms and all
updates for the TLS WS fixture. Under reordering the corresponding ages are
230.7, 5,561.6 and 701.3 ms; datagrams receive 352/360 updates with 31 arriving out
of order, and their reliable bulk takes 7.414 seconds. Content, stale-sequence
rejection, bulk and cleanup checks pass. Matching impairment seeds do not create
identical packet-loss trajectories; these single samples are not a statistical
ranking or evidence that buffer reuse caused a network-performance change.

The complete source/binary provenance, all medians/ranges and receipts are in
[the shared-transmit report](reports/webtransport-shared-transmit-2026-10-21.json).
Overall acceptance remains unmet. Continue worker/connection dispatch and mixed
private-lane scheduling, active QUIC/HTTP3 state, and datagram load measurement;
full observer/re-subscribe/cancellation, concurrent 100k/8192 inspection, broader
overload/shutdown, QUIC allocation accounting and final packaging/deployment
work also remain. This checkpoint does not accept, publish or deploy the pivot.

## Poll readiness more frequently and measure kernel socket storage

Tokio 1.53.3's multithreaded builder defaults to checking external events after
61 task polls. The CLI WebTransport runtime now checks I/O/timers after each
scheduled task (`event_interval(1)`). Native WS retains its separate Mio workers
and current-thread management runtime. Embedding callers that provide their own
runtime keep their own scheduling policy. No protocol, crypto, framing, FIFO,
resource limits or deadlines change. This is a workload tradeoff, not a fix for
mixed private-lane fairness or a general reduction in CPU cost.

The frozen executable is `artifacts/webtransport/binaries/event-every-task-relay`,
SHA256 `39285102fe520164af6db436a686ec608af1a4a8fd752f0957531da8726a2d24`.
The main source matches `event-every-task-main.rs` byte for byte, including after
rejection of the separate global-queue experiment. `target/release/war-relay`
has the same hash. The build manifest records the exact source diff and client
hashes. No formatter-only source change follows the benchmark freeze.

Two matched before/after blocks each alternate three fresh processes per case and
variant: four pinned server workers on CPUs 0–3, eight pinned client workers on
CPUs 8–15, four seconds, 1 KiB, 32 private pairs, 1,024 room readers, window four.
All 36 runs pass full content, sender, FIFO, counters, closure and cleanup checks.
The same fixed client is used throughout. No task-owned build, test or browser
runs alongside ordinary timing trials. Absolute rates vary substantially on this
shared host; keep the blocks separate rather than pooling or selecting rates.

| Reliable workload | Initial before | Initial after | Confirmation before | Confirmation after |
| --- | ---: | ---: | ---: | ---: |
| Private deliveries/s | 1,476,570 | 1,427,873 | 1,392,969 | 1,348,302 |
| Room deliveries/s | 1,366,005 | 1,587,857 | 1,159,479 | 1,378,602 |
| Mixed aggregate deliveries/s | 1,423,745 | 1,553,407 | 1,199,237 | 1,246,995 |

Room throughput improves 16.2% and 18.9%; mixed aggregate improves 9.1% and 4.0%.
Private-only throughput decreases 3.3% and 3.2%. Mixed private throughput changes
from 357,024 to 386,411/s initially, then 322,098 to 317,502/s in confirmation.
Thus its initial improvement does not reproduce; retain the policy for its
fanout/aggregate gains with the private-only and CPU costs explicit, and do not
present it as a private-lane fairness fix.

A separate **interleaved** WS/WT control uses the candidate executable's explicit
legacy mode for WS, otherwise the same verifier, placement and workloads. All 18
runs pass. It still contradicts full reliable parity:

| Interleaved protocol comparison | Plain WS deliveries/s | WT deliveries/s |
| --- | ---: | ---: |
| Private | 1,205,019 | 1,327,454 |
| Room | 1,319,460 | 1,157,606 |
| Mixed aggregate | 1,396,682 | 1,018,517 |
| Mixed private component | 681,714 | 269,816 |

Component and aggregate medians are independent. An earlier supplemental WS-only
block is preserved separately; do not substitute its rates into this comparison.
This is plaintext WS, not a native-WSS ceiling. Both server/client cycle profiles
are also captured together in separate eight-second mixed runs, including kernel
cycles. PID-filtered reports use **relative** percentages within each process;
absolute percentages of the combined recording would dilute those shares.
Both profiles pass the full verifier and have zero lost samples. Packet building,
crypto, stream/endpoint processing and synchronization remain visible. These
captures are excluded from ordinary medians and do not measure every host softirq.

The `global_queue_interval(1)` experiment is rejected and reverted. Against
I/O-every-task alone, three mixed runs show median aggregate rate falling from
1,369,189 to 724,038/s, private from 344,817 to 168,818/s. Its one private-only
and one room-only screen per variant are also retained, without pretending those
single samples form a complete matrix. The global-queue policy stays unchanged.

All six ordinary 100k before/after runs pass all 400,000 timed deliveries, all-user
pre/post proofs and retirement checks. Eight pinned server and eight client
workers, four offered seconds. Measured-gate median RSS is essentially unchanged:
2,813.91 versus 2,815.71 MiB. Achieved rate is 99,669.3 versus 99,640.0/s, with the
same 0.524 ms p50 and 2.097 ms p99 upper bounds in these samples. Server CPU rises
from 2.263 to 2.433 cores (7.6%); setup is 12.365 versus 12.594 s, retirement
2.458 versus 2.542 s. Timed UDP receive-drop deltas are zero, and all seven final
connection/storage gauges are zero. These latency samples differ from earlier
blocks and are not a production SLA.

The fixed direct-client datagram echo diagnostic also passes all six runs:
552,357 versus 551,531 received echoes/s, p50 56.079 versus 56.058 us, p99
159.187 versus 154.636 us. Server CPU per completion rises from 5.096 to 5.514 us
(8.2%). It uses 32 self-routing sessions with one outstanding echo each, eight
seconds, 1 KiB, four server/four client workers. It is not datagram saturation,
private/fanout/mixed acceptance, and its brief setup/teardown contributes to CPU.

### Separate kernel accounting

`scripts/socket_resources.py` reads Linux SOCK_DIAG's INET_DIAG_SKMEMINFO using
standard-library code and the installed UAPI layouts. It matches socket inodes
to process-owned fds, rejects different network namespaces, interrupted/truncated
dumps and duplicate sockets, and reports missing memory attributes explicitly.
It imposes no population/byte cap. The receive timeout is a diagnostic I/O failure
deadline. TCP state filtering distinguishes live connected sockets from listeners;
UDP reports owned bound sockets. Port-matched TIME_WAIT entries have no live fd
and are counted separately, without assigning object bytes.

Capacity adds optional `--kernel-socket-resources` and `--kernel-slab-sizes PATH`
flags. These snapshots run at quiescent phase gates and mark the entire run as a
**diagnostic**, so scan overhead cannot silently enter ordinary CPU/throughput
medians. Without the flags, no socket dump runs. Slab-size metadata can be captured
read-only with `sudo -n python scripts/socket_resources.py --slab-sizes-only`;
the receipt hashes the supplied metadata. Unprivileged reads report unavailable
slab sizes rather than replacing them with invented values. The helper can also
be run directly with `--pid PID --protocol tcp|udp --local-port PORT`.

The two 1,024-session smoke checks and two complete 100k diagnostic captures pass.
Both server and client are measured independently. On this kernel:

| Ready-gate kernel observation, per endpoint | WS | WT |
| --- | ---: | ---: |
| Owned data sockets | 100,000 TCP | 1 UDP |
| Slab object size | 2,432 bytes | 1,344 bytes |
| Owned socket-object estimate | 243,200,000 bytes (231.93 MiB) | 1,344 bytes |
| Receive allocation / queued transmit allocation | 0 / 0 | 0 / 0 |
| Forward allocation | 0 | 4,096 bytes |
| Missing memory attributes | 0 | 0 |

These paused-gate counters are not active-traffic peaks. Receive/send **limits**
are reported but are not allocations; some skmem fields overlap, so they are not
summed into an invented total. The slab object estimate excludes slab overhead,
other protocol allocations, routes/conntrack and other kernel resources. After
WS retirement, the server-owned connected count is zero but there are 100,000
port-matched TIME_WAIT entries. Their bytes are not estimated here. The two
endpoints' estimates must not be mistaken for one server's cost or counted twice.
The old approximately 25x process-RSS ratio is therefore not a complete-system
memory ratio. Kernel costs do not erase the still-large WT userspace footprint,
and these partial observations do not establish total-memory parity.

Four meaningful helper tests pass: queued UDP allocation and SO_RCVBUF match an
independent `ss` decode; IPv4 and IPv6 TCP queued data and endpoint ownership are
checked; malformed/truncated binary messages fail instead of undercounting.
All 266 relay tests pass (one existing ignored), all-target Clippy with warnings
denied and formatting pass, and vendor verification still covers 153 files.
Actual Chromium 156 passes all nine delivered-client checks, including datagrams,
typed values, 128 KiB/5 MiB messages, variables, departures and real close. Seven
final gauges are zero; the final screenshot was visually inspected.

All nine clean/loss/reorder freshness checks pass. Single samples per profile
retain the freshness benefit and explicit missing-update tradeoff: loss datagrams
have 73.4 ms p99 newest-state age and receive 346/360 updates, versus 2,221.3 ms
and all updates for the stream and 861.2 ms and all updates for the TLS WS fixture.
With reordering, ages are 97.8 / 5,941.4 / 681.4 ms; datagrams receive 352/360 and
their reliable bulk arrives after 7.022 s. Stale updates cannot replace newer
state, all bulk content is verified, and namespace cleanup succeeds. Matching
seeds do not mean identical lost packets, and these are not statistical rankings.

[Complete measurements and provenance](reports/webtransport-event-poll-2026-10-21.json)
include all blocks, rejected experiments, resource counters and scopes. Overall
throughput/memory parity remains unmet. Continue worker/connection dispatch and
mixed private fairness, retained QUIC/HTTP3 state and full datagram load measurement,
plus the previously listed observer/concurrent-inspection/overload/packaging
acceptance. No pivot acceptance, push, release or deployment is claimed.

### Release crypto staging storage when handshake keys retire

The established-connection heap showed surviving Initial/Handshake crypto
retransmission and assembler allocations. Quinn's key retirement cleared keys and
sent packets but retained those containers. The patch resets pending retransmits
and releases the assembler heap at that existing retirement boundary. Consumed
crypto offsets/order state and packet-number/ACK scalars remain intact. Retry
retains upstream ClientHello reconstruction and next packet number; the Data
space, stream credits, ciphers and congestion control are unchanged.

A fixed-client replacement-allocator diagnostic with 8,194 sessions reduced ready
live bytes from 233,425,785 to 227,135,902: 6,289,883 fewer, approximately 768 per
session. Two ordinary 100,000-session capacity comparisons, with reverse order in
the second block, reduced measured server RSS from 2,816.46 to 2,736.25 MiB and
2,813.54 to 2,734.06 MiB. Each run verified 400,000 timed messages, zero timed UDP
receive drops and all seven final resource gauges at zero. P50/p99 histogram upper
bounds remained 0.524/2.097 ms. CPU cores were 2.222/2.419 baseline and 2.272/2.362
candidate; setup 11.679/12.318 versus 12.038/11.903 s, retirement 2.394/2.537 versus
2.469/2.356 s. These gate RSS samples exclude kernel memory and are not continuous
peak measurements. Heap profiling is diagnostic and excluded from ordinary runs.

Three fixed-client runs per version/case produced median verified rates of
1,394,142 to 1,373,892 private messages/s, 1,452,207 to 1,451,676 room deliveries/s,
and 1,405,798 to 1,380,681 mixed deliveries/s. Mixed private medians were 354,103
to 348,993/s. Ranges overlap; no throughput improvement or statistical equivalence
is claimed. This is retained for its repeatable whole-workload memory reduction.

All 316 standalone protocol tests and 266 application tests passed (one existing
ignored test). A new owner-lifetime regression proves staging is released without
allowing consumed crypto bytes to be delivered again. The nine real UDP/TLS
integration checks pass, as do formatting, clippy and archive/patch verification.
[Complete measurements](reports/webtransport-discarded-crypto-2026-10-21.json)
include fixed binary hashes, latency/CPU/RSS/setup/retirement and cleanup evidence.
This candidate has not yet repeated browser or impaired-network freshness checks.
Memory remains about 2.67 GiB; the earlier native WS result was about 113 MiB and
is not a fresh cross-protocol comparison. Overall parity remains unmet. Continue
connection/packet-space storage and worker ownership, mixed private fairness,
full datagram private/fanout/mixed load and the remaining acceptance requirements.

### Materialize pending retransmission storage only when needed

`PacketSpace` embedded three full `Retransmits` containers in every connection,
including the retired Initial and Handshake spaces. Pending retransmissions now
use Quinn's existing `ThinRetransmits`, already used for sent-packet tracking.
Read-only inspection and merging absent queues allocate nothing. Mutation calls
`get_or_create` explicitly; a populated box moves into an absent destination.
Merging populated queues keeps upstream CRYPTO retransmit priority. Retirement
releases the pending box while packet-number/ACK/deduplication/assembler history
remains inline. No stream credits, cipher or congestion-control behavior changes.

The 8,194-session replacement-allocator diagnostic reduced ready live bytes from
227,126,986 to 223,459,858, about 448 bytes per session. Live allocations increased
by 8,213, approximately one additional Data-space owner per connection. In two
ordinary 100,000-session comparisons, RSS changed from 2,737.02 to 2,692.98 MiB and
2,734.64 to 2,692.85 MiB. Each run checked all 400,000 timed deliveries, had no timed
UDP drops, and ended with seven resource gauges at zero. P50/p99 histogram upper
bounds remained 0.524/2.097 ms. CPU was 2.339/2.325 baseline versus 2.401/2.408
candidate cores, roughly 3.1% more by median: an explicit memory/CPU tradeoff.
Setup was 12.363/12.365 versus 12.172/12.372 s; retirement 2.510/2.459 versus
2.501/2.651 s. Gate-sampled RSS excludes kernel memory and is not continuous peak.

Initial three-run medians changed from 1,346,008 to 1,349,975 private messages/s,
1,384,899 to 1,390,510 room deliveries/s, and 1,362,336 to 1,313,630 mixed
received deliveries/s. The independent mixed confirmation block changed from
1,424,832 to 1,444,822/s. Neither a consistent throughput improvement nor penalty
is established; statistical equivalence is not proved. Both blocks are retained,
not combined with earlier protocol-control measurements. The memory reduction
is retained with its observed CPU cost; it does not satisfy overall parity.

After rustfmt reflow/trailing commas, the separately hashed final build passed
another ordinary mixed check (1,517,927 verified deliveries/s) and capacity run
(2,694.78 MiB RSS, 2.332 CPU cores, all 400,000 messages checked, seven final gauges
zero). These single final-build checks are not matched performance comparisons.
All 317 protocol and 266 application tests pass (one existing ignored test), plus
clippy, formatting and complete vendored archive/patch provenance. A new merge
regression checks allocation transfer and CRYPTO priority. Actual Chromium 156
passed all nine delivered-bundle checks, including typed values, explicit
datagrams, 5 MiB payload, concurrent controls and genuine close; seven gauges
returned to zero and the screenshot was inspected.

All nine clean/loss/reorder freshness cases passed. In the single loss cases,
datagram p99 state age was 73.1 ms with 347/360 updates; reliable WT was 701.2 ms
and TLS WS 901.9 ms, both receiving all updates. Datagram reliable bulk arrived
in 738 ms. With reordering, datagram age was 123.6 ms with 353/360 updates and
7.310 s bulk; reliable WT was 5,761.5 ms with 5.825 s bulk; TLS WS was 840.9 ms
with 877 ms bulk. This retains the freshness benefit and exposes the bulk
tradeoff; single seeded trajectories do not establish statistical rankings.

[Full measurements, hashes and validation](reports/webtransport-thin-pending-2026-10-21.json)
retain every run and its scope. Overall memory remains about 2.63 GiB versus the
earlier native WS result around 113 MiB, not a fresh cross-protocol comparison.
Continue the worker/connection ownership redesign and mixed fairness, full
datagram private/fanout/mixed saturation, remaining observer/concurrent/overload
acceptance, complete kernel peak accounting and final packaging requirements.
No pivot acceptance, push, release or deployment is claimed.

### Reprofile mixed traffic and reject deferred timer resets

Fresh dual-endpoint mixed profiles on retained `45a0aeb` sampled user and kernel
cycles with PID-relative attribution. QUIC server samples were 56.95% in the
relay/library binary, 33.33% kernel and 8.49% libc; native WS was 8.56% binary,
90.85% kernel. Client shares also differed: QUIC 57.41% binary/32.62% kernel,
WS 24.39% binary/72.49% kernel. These relative shares are not absolute service
costs, and the saturating workloads have different delivered traffic composition.
QUIC self costs are distributed: AES-GCM encryption, packet construction, stream
reads, Notify/waker/scheduler, memory routines and kernel UDP processing. No
single dominant removable function is established.

A bounded experiment retained an already armed earlier wake when a logical
protocol deadline moved later, rearmed immediately for earlier deadlines, and
used the protocol clock to prevent premature or delayed timeout reporting. A
clock/wake regression plus 29 standalone Quinn tests passed (three existing
ignored tests). The final standalone fixture explicitly selected patched local
proto 0.11.19, Tokio 1.53.3, rustls 0.23.46 and bytes 1.12.1; earlier registry-graph
results are not final graph evidence. The project lockfile was unchanged.

The timer-reset symbol went from 1.18% to below the 0.3% displayed self-symbol
threshold, but the repeated ordinary whole-workload rates did not improve.
Private medians were 1,393,350 to 1,386,812/s, room 1,473,998 to 1,456,057/s,
and mixed 1,395,792 to 1,367,851/s (mixed private 350,791 to 346,342/s). Component
profile shares cannot prove an absolute reset-cost reduction, and the measured
rates do not support retaining the experiment. Both tracked dependency files
were restored byte-identically; the added timer module was removed. The binary
in target/release was restored from the frozen retained build and hashes match.
Complete 153-file archive/patch verification passes. Rejected source, patch,
binary and test/measurement receipts remain local for reproducibility.

An independent unprofiled three-run native-control block with alternating order
now gives WT/native WS medians of 1,432,929/1,439,602 combined deliveries/s,
360,812/776,191 private messages/s, and 1,072,117/621,167 room deliveries/s.
Component medians are independent and need not sum to the aggregate median.
Server CPU was 3.476/2.106 cores, private p99 histogram upper bounds 4.194/2.097 ms.
This is near aggregate parity with a major private-traffic deficit and higher
observed CPU use, not mixed parity. Different delivered composition prevents
calling the CPU ratio an equal-per-message cost comparison. All content/order,
close/counter and final-gauge checks pass. Profiles are excluded from these rates.

The first WS profile lost 143 samples and was excluded. A clean retry produced
zero-lost reports for both PIDs. The requested lower frequency in its Namespace
was ignored by inspection.py's hardcoded perf command: all retained profiles
actually use 499 Hz, and the report records the actual setting. The original
and candidate QUIC profiles also have zero lost samples.

[Complete profile/control/negative-experiment evidence](reports/webtransport-worker-layout-2026-10-21.json)
records hashes, all ordinary samples, latency/CPU/RSS, both endpoint DSO shares,
validation and limits. No product optimization is retained from this experiment.
The evidence supports moving the next prototype to worker-owned protocol and
application processing rather than accepting timer-only wins; that redesign is
not implemented or proven faster. Full datagram saturation/fanout/mixed measurement
and all previous memory/behavior/packaging/kernel acceptance requirements remain.
The goal remains active; no pivot acceptance or release is claimed.

### Co-drive application and connection I/O: reject regressions, identify batching cost

An opt-in prototype transferred Quinn connection driving from its background task
to the existing application session task. Driver epochs retired stale background
futures, ownership began only on first poll, and completion/cancellation restored
background I/O for retirement. Protocol implementation, TLS, stream credits,
mutex/event ownership and transport settings stayed intact. This was a partial
integration experiment, not a worker-owned protocol implementation.

Three independent fixed-client comparison blocks, each three fresh alternating
runs per variant and private/room/mixed workload, rejected all variants:

| Prototype | Private/s before → after | Room deliveries/s before → after | Mixed deliveries/s before → after | Mixed private/s before → after |
| --- | ---: | ---: | ---: | ---: |
| I/O before and after application poll | 1,386,200 → 1,397,209 | 1,478,353 → 1,086,799 | 1,413,165 → 1,104,734 | 356,246 → 342,606 |
| Skip idle I/O with valid readiness | 1,427,260 → 1,437,616 | 1,571,671 → 1,182,180 | 1,541,535 → 1,233,396 | 383,653 → 376,368 |
| Defer post-write transmit to scheduled turn | 1,419,653 → 1,413,076 | 1,568,947 → 1,195,325 | 1,527,489 → 1,264,296 | 380,227 → 425,517 |

These are block-specific medians, not cross-block rankings; component medians
need not sum to mixed aggregate medians. Common contract: four server workers
pinned CPUs 0–3, eight client workers CPUs 8–15, 4s, 1 KiB values, 32 private pairs
and/or 1,024 room readers with four updates in flight. All independent content,
sender, FIFO, counter, close and final-gauge checks passed in these ordinary runs.
The deferred version shifted service toward private traffic but still lost about
24% room and 17% aggregate mixed throughput. No variant satisfied acceptance.

The initial idle-readiness gate failed the large-fanout integration test. An
empty event queue could lack a reader registration after a cooperative yield;
a stale idle waker could also suppress quantum-limited continuation. The corrected
gate required valid matching wake registration, handled buffered sends/deadlines,
and cleared the idle marker during active driving. All nine real UDP/TLS/HTTP3
integration checks then passed, including for the deferred variant. Thirty
standalone Quinn tests passed (three existing ignored), covering registration,
duplicate ownership, an unpolled wrapper, cancellation, completion and resumed
background delivery. This is not general-purpose shutdown/unwind acceptance.

A separate retirement-counter diagnostic compared instrumented retained/deferred
builds. It read existing Quinn statistics once per Output retirement, adding no
per-packet counter updates. Every run accounted for all 1,025 room-only or 1,089
mixed sessions. For the three room/window-four runs, median datagrams per transmit
batch fell from 2.244 to 1.860; wire bytes per batch fell from 2,684 to 1,926.
Generated transmit batches per wire MiB rose from 390.71 to 544.49, about 39%.
This identifies lost packet filling/batching as one mechanism. The single
window-one pair had exactly one datagram per batch for both versions but still
favored the retained implementation, so batching does not explain all overhead.
These are Quinn-generated batch counters, not independent kernel syscall counts.
They cover handshake/application lifetime and warmup until Output retirement,
exclude subsequent draining, and are not pure timed per-message counters. All
instrumented runs are marked diagnostic and excluded ordinary rate medians.
No MTU override, added sleep or client change was used.

Separate profiles sampled both server and client for retained/deferred builds at
249 Hz with user+kernel cycles, DWARF stacks and zero lost samples for both PIDs.
Server self samples for ConnectionDriver::poll rose 2.03% → 2.67%, Notify polling
1.43% → 2.10%, and packet population 2.37% → 2.70%; these relative shares are not
absolute per-message costs. The profiles and counters support preserving explicit
readiness and packet batching in the next worker-owned design, not simply merging
arbitrary futures and assuming that fewer tasks are faster.

All six changed product/dependency source files were restored byte-identically
from retained HEAD; the frozen retained binary was restored in target/release,
and archive/patch verification passes for all 153 vendor files. No prototype API,
temporary counter or diagnostic admin response remains. Rejected source patches,
binaries and receipts remain local. No 100,000-session memory saving, browser or
freshness acceptance is claimed for these rejected prototypes. The retained
implementation and its prior verification remain unchanged.

[Complete prototype, counter and profile evidence](reports/webtransport-application-driver-2026-10-21.json)
records all three blocks, resource/latency measurements, hashes, failed and
corrected checks, packet statistics and scope. Overall parity remains unmet.
Continue worker-owned explicit input/application/transmit phases, full datagram
private/fanout/mixed load, 100k memory parity and the remaining behavior, kernel
and packaging acceptance. The goal remains active; no pivot acceptance is claimed.

### 2026-10-21: matched open-loop load and reliable scheduling diagnosis

Added a maintained verified open-loop client and runner ([contract and commands](datagram-load.md)). The [90-row evidence report](reports/webtransport-datagram-load-2026-10-21.json) retains all failed reliable rows as well as successful observations. These are paced offered-load points, not maximum-capacity proofs. The provisional scheduling binary and the final actual-Pending idle-wait amendment are explicitly distinguished. Original receipts retain hashes, complete recipient results, process resources, and logs.

The retained implementation serves approximately 1.02 million verified private datagram deliveries per second at the corresponding offered point, and approximately 1.02 million fanout deliveries per second separately. High mixed load remains deficient: the final scheduling candidate delivered 85,074 private and 991,213 fanout deliveries per second, with 651,432 private and 116,447 fanout updates unreceived within the observation window. A successful integrity/cleanup receipt does not mean the offered traffic was all delivered. Reliable high private and mixed runs still reject slow consumers. Throughput parity is not achieved.

A distinct deterministic scheduling bug was reproduced with a coalesced reliable self-route burst: input could exhaust admission before its session writer ran. Writer-first polling and queue-derived bounded work quanta fix this case without raising queue or byte budgets. A fresh writer quantum is allowed only after an actual Pending empty-queue wait, preserving the fairness bound when a notification races. The final implementation passes the 10 WebTransport integration tests, the complete all-target test suite, and Clippy with warnings denied. Broader cross-peer overload remains unresolved.

The provisional candidate's 100,000-session RSS was 2,694.63 MiB versus 2,691.99 MiB for the retained implementation in one matched pair; both checked 400,000 deliveries. This is not statistical memory equivalence or memory parity with native WebSocket. The final amendment requires its own resource and whole-workload confirmation. Goal remains active; no release or acceptance is implied.

Final-binary confirmation ([receipts](reports/webtransport-reliable-scheduling-2026-10-21.json)): 1,472,179 private deliveries/s; 1,473,816 fanout deliveries/s; mixed 352,737 private plus 1,040,431 fanout deliveries/s. These are single confirmations using the fixed reliable client, not repeated comparative gains. At 100,000 sessions the final binary used 2,694.79 MiB measured RSS, verified all 400,000 measured deliveries, and retired with zero final resource gauges. Final-binary browser and loss/freshness verification remain pending. All-target tests: 270 passed.

Final-binary follow-up: all nine clean/loss/reorder freshness cases passed; [results](reports/webtransport-final-freshness-2026-10-21.json). Under loss, datagram p99 state age was 66.78 ms with 16/360 updates unreceived, versus reliable-stream 641.49 ms with none missing. The reorder reliable-stream single run stalled to 5,940.85 ms p99 state age; preserve this outlier for investigation rather than inferring a typical ranking. Browser verification passed all nine representative checks, including datagrams, exact IDs, variables, 5 MiB payload, and normal closure; screenshot inspected and seven cleanup gauges zero.

[Paired open-loop server/client profiles](reports/webtransport-open-loop-profiles-2026-10-21.json) completed at 249 Hz with zero lost samples. Private-only diagnostic: 1,023,903 deliveries/s and zero socket drops. Mixed diagnostic: 87,122 private/s plus 987,024 fanout/s, with 145,800 server socket receive drops. Fanout adds kernel socket/spinlock and connection-driver sample cost. This supports testing dedicated ingress servicing, but neither sample shares nor receive drops alone prove the scheduling cause. No architectural change is claimed from these observations.

### 2026-10-21: independently service the shared QUIC endpoint

The [repeated comparison and final executable receipts](reports/webtransport-dedicated-ingress-2026-10-21.json) identify a causal integration bottleneck. Constructing the endpoint on an owned current-thread runtime lets its UDP reactor and EndpointDriver progress independently of the application/connection run queue. Connection acceptance still spawns connection drivers on the existing application runtime. Four-worker comparisons retain CPUs 0–3 for the entire server, with one additional thread sharing those CPUs; no queue, byte budget, or receive-buffer limit changes.

Three rotated prototype pairs at the identical high mixed datagram offer show private median 86,502 to 255,788/s and fanout 990,389 to 1,023,098/s. Prototype input socket drops fall from 66,160–74,918 to zero, and all generated updates reach every recipient. Observation-average server CPU remains about 2.6 cores. This separates endpoint servicing as a useful causal change, but moves the reactor and dispatch task together; their individual contributions are not isolated.

The integrated executable confirms private 255,840–255,878/s and fanout 1,023,026–1,023,204/s across three runs, all with zero server socket drops and zero private missing. One run misses 57 fanout deliveries, with 35 client UDP drop units; the server receives/submits all generated traffic in that run. The other two have none missing. Do not equate UDP drop units with application message counts or call this universally loss-free.

Preserved tradeoffs: in the repeated prototype block, reliable private-only median falls 1.62% (1,377,663 to 1,355,376/s), fanout falls 1.95% (1,409,460 to 1,381,912/s), and reliable mixed aggregate rises 3.24% (1,318,710 to 1,361,462/s). Integrated fixed-client confirmation covers each workload. The high paced private-stream case still fails 3/3, and high mixed streams fail 2/3 with slow-consumer counters; one successful run is not stable parity. Datagrams and reliable streams remain separate contracts.

Both prototype 100,000-session cases verify 400,000 measured deliveries and final resource cleanup. RSS is 2,693.88 versus 2,692.81 MiB; the integrated executable uses 2,691.66 MiB in its own confirmation. These are single resource comparisons, exclude kernel memory, and do not show memory parity with native WebSocket. A separate 8,194-session replacement-allocator diagnostic has ready live bytes 223,450,666 versus 223,473,133 (+22,467 shared bytes); it is not a 100,000-session heap result or an ordinary timing measurement.

The integrated component passes 271 all-target tests, Clippy with warnings denied, and 153-file vendor verification. All WebTransport integration fixtures now exercise the independent endpoint runtime, including bounded scheduling, pressure, idle deadlines, large fanout/departures, and cleanup. A bind-failure regression checks startup rollback. Actual Chromium passes all nine browser checks; screenshot inspected and seven cleanup gauges zero. All nine clean/loss/reorder freshness cases pass. Under loss, datagram p99 state age is 69.47 ms with 14/360 unreceived, versus reliable-stream 580.75 ms with none missing; single cases are not statistical rankings. The long reliable reorder stall remains visible in the report.

The integrated mixed server/client profile has zero lost samples. Its receive drops are zero, but diagnostic traffic still has missing updates; profiles are excluded from ordinary timing claims. Endpoint servicing is retained for the measured complete mixed-workload gain. Reliable admission pressure, private/fanout maximum-capacity parity, and the much larger per-session memory deficit remain open; goal not complete.

### 2026-10-21: reject per-record reliable input yielding

The [fixed-client experiment](reports/webtransport-reliable-input-quantum-2026-10-21.json) changes only the reliable input quantum to one record, leaving writer quanta, queues, byte budgets, and datagrams intact. High paced mixed streams pass all three trials, but high private streams still fail all three with slow-consumer rejection. This is insufficient as a general reliable-admission fix.

Three rotated complete-workload pairs show private median 1,367,883 to 734,599/s (-46.30%), fanout 1,385,101 to 1,372,273/s (-0.93%), and mixed aggregate 1,338,046 to 1,165,321/s (-12.91%). All 18 windowed observations verify data/FIFO and cleanup. Preserve the six paced results including the failures; do not substitute successful mixed trials for a general throughput gain. The experiment is rejected. Source and target/release/war-relay are restored to the retained aaadb0a implementation and frozen binary.

Source inspection identifies the always-queued reliable output handoff as a candidate for a native-style immediate private/control write path. Fanout should preserve deferred batching, and partial writes must retain FIFO, offsets, reservations, and deadlines. That path is not implemented or proven by this diagnostic. More frequent producer yields alone do not distinguish an unpolled writer from QUIC congestion/flow-control blocking. Goal remains active.

### 2026-10-21: immediate reliable private/control output

The [fixed-client comparison and final verification](reports/webtransport-immediate-output-2026-10-21.json) removes a queue handoff from idle reliable private/control output. A single cancellation-safe chunk write is polled synchronously under the submission gate. Complete writes release the existing reservation normally. Partial or blocked writes retain the owned envelope, accepted-byte offset, and first-partial-write deadline. Dequeue transfers exclusive stream ownership under the same queue gate, preventing overtaking before the writer's first poll. Fanout and gated observer output remain queued for batching. No queue limit, byte budget, input quantum, or client binary is raised or optimized.

Three rotated prototype comparison pairs show reliable private median 1,369,584 to 1,432,096/s (+4.56%), fanout 1,353,804 to 1,349,661/s (-0.31%), and mixed aggregate 1,336,832 to 1,308,828/s (-2.09%). Retain those costs rather than claiming a universal win. In return, all six prototype paced high-load stream cases pass. The final executable separately passes all six: private 1,023,566–1,023,764 verified/s; mixed private 255,676–255,848/s plus fanout 1,022,580–1,023,021/s. Every generated reliable update arrives FIFO. Previously these private cases failed3/3 and mixed failed2/3 on the retained ingress binary. These are offered-load points, not proof of equal maximum capacity.

All three final high mixed datagram confirmations retain zero missing generated updates, at private 255,799–255,894/s and fanout 1,023,078–1,023,414/s. Datagrams remain explicitly unreliable. The final 100,000-session case verifies400,000 deliveries and clean retirement, but measured RSS is2,714.66MiB versus earlier ingress confirmation2,691.66MiB. A fresh8194-session replacement-allocator comparison increases ready requested live bytes223,485,196 to226,371,030 (+2,885,834). Neither constitutes memory parity; the new stream-ownership handoff and pending-output metadata are candidates for reducing this cost. Single RSS confirmations are not a statistical memory comparison.

The final executable passes274 all-target tests, Clippy warnings denied, and153-file vendor verification. New tests reconstruct every possible offset across record/routing headers and body, verify ownership transfer under the queue gate, and resume a12MiB partial record ahead of a following record. Existing timeout/cancellation tests retain charged credit until completion or abort. Actual Chromium passes all nine browser checks including5MiB payload and cleanup; screenshot inspected and seven gauges zero. All nine clean/loss/reorder freshness cases pass. Under loss, datagram p99 state age77.50ms with14/360 unreceived versus reliable-stream2301.40ms with none missing; single trajectories do not establish a statistical ranking or unchanged reliable bulk behavior. The long reorder stream stall remains present.

Both endpoints were profiled in separate reliable mixed before/after diagnostics at249Hz with zero lost samples, plus a final datagram diagnostic. Profiles are excluded from ordinary timing. The deadline-duration amendment and final binary confirmations are separate from the repeated prototype cohort. The admission improvement is integrated with its measured throughput and memory costs. Full maximum-throughput and memory parity remain unachieved; goal stays active.

### 2026-10-21: borrow the stream under its ordering lock

The [ownership comparison](reports/webtransport-writer-ownership-2026-10-21.json)
keeps the stream in queue-owned state and marks a dequeued record busy under the
same lock. Each write poll borrows it briefly. Pending chunk suffixes remain in
the future, so accepted bytes are not replayed; the lock is released before
awaiting. No stream is copied into each parked writer future. No queue limit,
byte budget, input quantum, or maintained client changes. All 274 tests, Clippy,
153-file vendor checks, nine browser checks and nine freshness cases pass.

A fresh ordinary 100,000-session pair reduces RSS from 2,714.20 to 2,697.14 MiB,
with all 400,000 measured deliveries verified in both and complete cleanup. A
separate 8,194-session allocation pair reduces requested live bytes from
226,355,770 to 223,213,974. Three rotated windowed pairs give private median
1,431,523 to 1,443,903/s (+0.86%), fanout 1,353,947 to 1,364,167/s (+0.75%), and
mixed aggregate 1,342,208 to 1,317,022/s (-1.88%). The ranges overlap; this is a
memory improvement with observed throughput tradeoffs, not a universal speedup.
All six high paced reliable cases still pass with every generated update FIFO.

Original-client datagram tests retain small missing counts in some candidate
runs, coinciding with client UDP receive drops. The server receives/submits all
generated traffic and has zero receive drops. Paired original-client controls
and a fresh two-server/two-client scheduler experiment are preserved in the
report. Changing only client event_interval(1) is not a reliable fix and is not
retained: the maintained example source and binary are restored. Client changes
are not used to claim a server improvement. All nine loss/reorder cases pass;
under loss, datagram p99 state age is 70.30 ms with 10/360 unreceived versus
reliable-stream 1,221.36 ms with all received. Single trajectories are not
statistical rankings; the long reliable reorder stall remains visible.

### Full-scale allocation and kernel accounting

The [100,000-session memory report](reports/webtransport-100k-memory-2026-10-21.json)
now contains complete native WebSocket and WebTransport live-heap captures,
ordinary allocator observations, setup/retirement and socket accounting. Both
use the same frozen server/client executables with the selected transport,
100,000 sessions, 6,250 rooms, 1 KiB payloads and 400,000 verified deliveries.
Fresh ordinary native RSS is 113.79 MiB versus WebTransport 2,697.14 MiB (23.70x).
Requested live allocations at ready are 118,839,363 versus 2,648,845,537 bytes
(22.29x). The profiler's RSS and timings are separate diagnostics, not ordinary
performance. Memory parity is disproven for this measured contract.

Connected TCP socket-object estimates are 243,200,000 bytes per endpoint,
reported separately from process memory. The shared server UDP socket object is
1,344 bytes. Buffer limits are not allocations and these estimates exclude other
kernel metadata/overhead; this is not complete system memory accounting. The
initial collector's five-second timeout left a QUIC retirement capture
incomplete; that failed run remains saved. The maintained collector waits for
SIGUSR2 handler completion and validates all five gates for both transports.
At logical done, native requested storage is about 1.88 MB and QUIC about
70.39 MB; process-exit profiles are separate and much smaller. Do not label
retained RSS or delayed release a leak from those gauges alone.

The [verified allocation-site breakdown](reports/webtransport-100k-heap-sites-2026-10-21.json)
attributes 641.6 MB to one 6,416-byte endpoint-accept allocation per connection,
about 230 MB to packet-key allocations, and 116 MB to packet-tracking allocations.
It identifies the first captured executable frame, not a complete optimized
call stack or a proven type layout. These larger retained structures are the
next memory targets. The ownership improvement is retained with the measured
costs; full throughput and memory parity remain unachieved, and the pivot is not
accepted. Reproduction commands and the maintained heap collector/analyzer are
in the [performance guide](performance-guide.md#phase-gated-capacity-allocations).

### Empty sent-packet tree reclamation — 2026-10-21

The [controlled comparison and receipts](reports/webtransport-empty-packet-tree-2026-10-21.json)
retain a five-line change to release the sent-packet BTreeMap root when the last
outstanding packet retires. Packet-number, ACK/loss and deduplication history
remain intact. The ordinary 100,000-session pair reduced ready RSS from
2,696.32 to 2,592.88 MiB (103.43 MiB); both completed 400,000 checked deliveries
and cleanup. This is one resource pair, not a confidence interval.

There is a measured throughput cost. In the rotated three-way block, retained
versus selected median verified deliveries/s were 1,446,507 versus 1,439,217
(private), 1,364,003 versus 1,332,460 (room), and 1,353,307 versus 1,341,964
(mixed): -0.50%, -2.31%, -0.84%. The separate initial pair block measured
-0.68%, -1.35%, -2.58%; cohorts remain separate. A worker-local empty-root cache
was rejected: its mixed median was 1,290,517/s. The simple policy saves idle
memory at a small active-throughput cost; it does not prove parity.

The completed full-scale replacement-allocator diagnostic captured all five
gates. Ready requested live bytes fell from the previous 2,648,845,537 to
2,528,858,001. All buckets resolve to the exact binary; the 641.6 MB accept-site
allocation and approximately 230 MB packet keys remain major targets. This
attribution identifies executable allocation sites, not proven Rust type sizes.
Requested allocations and profiler RSS are not ordinary resource measurements.
The initial incomplete capture is preserved. The collector now retries signal
requests intentionally ignored by gperftools' TryLock handler, with per-gate
attempt counts; the real-profiler recovery check passes. Logical done and exit
profiles remain separate.

Verification: 274 relay tests passed (one existing ignored), 317 quinn-proto
tests passed, Clippy passed, and 153 vendor files verified. Six high-paced
reliable cases, six paired datagram cases, nine loss/reordering fixture cases,
and all nine actual Chromium checks completed. Datagram integrity does not mean
loss-free delivery: paired runs retain their unreceived counts and client UDP
drops. Browser cleanup returned all session/connection/storage gauges to zero;
the permanent global channel remained at one. The browser also verified a
5 MiB application message. Full memory and throughput parity remain unachieved.

### Rejected lazy migration history — 2026-10-21

The [migration representation experiment](reports/webtransport-lazy-migration-rejected-2026-10-21.json)
boxed the optional previous path instead of reserving it inline. The measured
accept-site allocation fell from 6,416 to 5,984 bytes per connection, exactly
43.2 MB at 100,000 sessions. The ordinary matched pair reduced ready RSS from
2,595.05 to 2,552.69 MiB; each completed 400,000 checked deliveries and retirement.
Three-run median reliable throughput changed +0.64% private, -1.88% room and
+0.61% mixed, with substantial variation. Compiler layouts are separate
standalone default-feature diagnostics; the exact relay heap confirms the
allocation change independently.

The candidate is rejected for now. Five of six initial high-paced reliable
cases passed; private case three failed with code 1013 and 20 slow-consumer
closures, with no global-pressure events. The driver continued after that
failure; its process exit was not proof of all cases passing. Three subsequent
baseline and three candidate private runs passed. Both builds also passed the
targeted private profiles, with zero lost samples on both endpoint reports.
These controls did not reproduce or explain the initial failure. This does not
prove the representation caused it, but is insufficient evidence to accept the
candidate under the high-rate reliable workload requirement. The failed receipt
is retained and is not replaced by successful retries.

Other completed candidate checks: 317 protocol tests, the full relay suite,
Clippy, 153 vendor files, six paired datagram cases, nine loss/reordering cases,
and all nine actual Chromium checks including a 5 MiB message and resource
cleanup. Both endpoint mixed profiles also had zero lost samples. Datagram
missing updates and client UDP drops remain in the report. Under the loss
fixture, datagram p99 freshest-state age was 69.78 ms with 14/360 missing;
reliable stream was 1,020.85 ms and TLS WebSocket 940.60 ms with no missing.
Single fixture cases are not statistical rankings. Profiled timings and heap
allocator RSS are excluded from ordinary comparisons.

Source and complete vendor patch were restored byte-for-byte to commit 1277121;
the selected relay binary is again SHA256
`bc23c4bc6e5cee7b03f37c852883c3798a8da6011beb68a57874f4c7a35521ce`.
No migration representation change is retained. Memory/throughput parity remain
unachieved. The remaining inline packet-space, stream, crypto and runtime state
are much larger targets than this optional migration structure.

### Retained QUIC-only TLS record cipher omission — 2026-10-21

The [record-cipher investigation](reports/webtransport-quic-record-ciphers-2026-10-21.json)
identified 100,000 TLS record encrypters and 100,000 decrypters retaining 112 MB
in addition to QUIC packet keys. Rustls queues QUIC handshake bytes without TLS
record encryption, but uses record-layer active state to label encryption
levels. The narrow patch preserves that state, sequence resets and limits using
rustls' existing fail-closed zero-sized sentinels only for QUIC. TCP/TLS keeps its
original cipher derivation; QUIC packet/header protection, authentication,
handshake secrets, key logging, algorithm selection and key updates are intact.
Published rustls 0.23.46 is checksum-pinned with a declared one-file patch.

The ordinary matched 100,000-session pair reduced ready RSS from 2,592.40 to
2,482.66 MiB, saving 109.74 MiB. Both completed 400,000 checked deliveries and
retirement. This is one resource pair, not a confidence interval. All five heap
gates completed; ready requested live bytes were 2,425,751,021, and the record
cipher allocation sites disappeared. Profiler allocator RSS/CPU/latency remain
excluded from ordinary comparisons. Separate socket accounting is incomplete;
limits and kernel counters are not added to RSS.

Throughput cohorts disagree. The initial three-run pair block measured -0.29%
private, +2.28% room and -4.63% mixed. A rotated 27-case comparison included the
committed build, an unmodified rustls build from the same vendor path and the
candidate. Candidate medians were 1,438,244 private, 1,343,050 room and 1,310,490
mixed verified deliveries/s: +0.99%, +1.00%, +2.48% vs committed, and +0.56%,
+0.94%, +4.28% vs vendor control. All used the same fixed client. Both cohorts
are preserved; no consistent throughput gain is claimed. The initial control
launch lacked executable permission after copying; no server started. Its log
is preserved, permissions were corrected without byte changes, and only a
hash/arguments-verified passed receipt was reused on resume.

All six high-paced reliable cases passed with no unreceived application messages.
All six paired datagram cases passed integrity/cleanup; the candidate's first
run had 5,844 private and 13,948 fanout updates unreceived, with 5,604 client UDP
drops. Two later candidate runs had no missing updates. Server UDP receive drops
were zero throughout. These results are not labeled loss-free. All nine
clean/loss/reorder fixture cases passed. Under loss plus bulk traffic, datagram
p99 state age was 77.32 ms with 16/360 updates missing, versus 720.90 ms for
reliable stream and 1,061.05 ms for TLS WebSocket with none missing. Datagram bulk
arrival was 2,143.94 ms. These are single fixture cases, not statistical rankings.

Verification includes 274 relay tests, 247 rustls unit tests, 317 Quinn protocol
tests linked to the patched rustls, Clippy and 272 vendor files. Both endpoint
profiles had zero lost samples. Actual Chromium passed all nine checks including
5 MiB delivery; session/connection/storage gauges retired, with one permanent
global channel. The published rustls crate omits test fixtures and fuzz corpus;
`scripts/test-rustls-vendor.py` verifies the exact upstream archive checksum and
runs all unit tests in a fresh copied project, with locked dependencies. The
maintained helper itself passed all 247 tests. No tests were skipped to get the
passing result; initial fixture failures remain recorded.

Retain this as a verified memory improvement. The selected relay SHA256 is
`773479940aa1498da90dd62ef3e2b9236e34fb412a2f41b5fba7477b98a8a3f2`.
Requested storage remains about 2.426 GB against the earlier native baseline of
about 119 MB. Full throughput and memory parity remain unachieved; the pivot is
not accepted. No push, deployment or PR is part of this change.

### Rejected borrowed QUIC record bodies — 2026-10-21

The [ordered-chunk reader experiment](reports/webtransport-chunk-reader-rejected-2026-10-21.json)
returned an immutable QUIC body chunk directly when it contained the complete
record body, avoiding a new Vec and copy. It preserved explicit four-byte
framing, ordered reads on the same stream, maximum-size checks, the same full
announced-size staging lease, bounded assembly and cancellation ownership. It
never consumed ahead of the current record. All 274 tests and Clippy passed;
both 100,000-session cases completed 400,000 deliveries and clean retirement.

Reject the change: the initial four-second cohort's +0.82% private, +1.66% room
and +1.24% mixed medians did not persist in the separate twelve-second cohort.
Long baseline/candidate medians were 1,417,723/1,422,197 private (+0.32%),
1,348,535/1,332,929 room (-1.16%), and 1,319,563/1,316,336 mixed (-0.24%). Every
long room candidate sample was below every baseline sample. No repeatable
whole-workload throughput gain was established. All 36 traffic runs passed;
passing correctness does not establish an optimization.

The ordinary capacity pair used 7.87/7.52 server CPU seconds but candidate ready
RSS increased about 3.45 MiB. This single pair is not a confidence interval. The
compiler diagnostic left session/spawn futures unchanged at 1,736/1,808 bytes;
the original 192 MB task-allocation target was not reduced. Server/client CPU
sets were checked against actual topology and use distinct physical cores.

Experimental source and binaries remain archived locally; source and selected
release binary were restored to commit 874983e, SHA256
`773479940aa1498da90dd62ef3e2b9236e34fb412a2f41b5fba7477b98a8a3f2`.
No reader change is retained, and no new browser/loss acceptance is claimed for
the rejected candidate. Full throughput and memory parity remain unachieved.

### Phase-gated packet and ACK census — 2026-10-21

The [protocol census](reports/webtransport-protocol-census-2026-10-21.json)
used diagnostic-only binaries; the selected implementation and release binary
were restored before runs. A weak session registry and read-only local counter
endpoint enabled snapshots while the fixed client paused at ready/measured
gates. Both snapshots had exactly the same live connection IDs and expected
population. Diagnostic timings/RSS are excluded from ordinary comparisons.
All five cases passed integrity and cleanup.

Measured-window received ACK-frame/datagram ratios were 8.76% private, 97.35%
room fanout, 53.03% mixed reliable, 7.42% paced private and 60.38% paced mixed
datagrams. These are frame/datagram ratios, not CPU shares or necessarily a
packet-type partition. No detected packet loss or data/stream-data blocking was
recorded in those windows. Quinn send datagrams/I/O-group ratios were 4.22,
2.67, 2.99, 6.36 and 2.69 respectively; batching already exists. These I/O
counters do not count kernel syscalls. Small ACK tails before the measured-gate
snapshot remain included.

An initial lifetime census is preserved separately. It includes substantial
room-departure notification fanout during cleanup; it must not be mistaken for
steady data traffic. Datagram submissions and verified arrivals are different
quantities. The phase-gated results support testing ACK-frequency requests on
fanout/mixed workloads, while providing a private-routing control. They do not
yet prove an ACK-policy change saves CPU or improves complete workloads.

### Retained eight-packet ACK request — 2026-10-21

The [ACK-frequency report](reports/webtransport-ack-frequency-2026-10-21.json)
retains a server transport factory used by both CLI and integration fixtures.
It requests acknowledgement after eight ack-eliciting packets, with a 5 ms
maximum delay and immediate acknowledgement of reordering. Unsupported peers
ignore it. Custom endpoint callers retain their supplied configuration. Queue,
read, stream-credit, fairness, deadline and application reliability rules remain
unchanged. No diagnostic registry or counter endpoint is retained.

Separate phase-gated counters confirmed one request sent for every live measured
connection. Received ACK frames per verified application delivery fell from
0.264 to 0.105 in room fanout (about 60%), 0.216 to 0.113 in mixed reliable
(about 48%), and 0.304 to 0.140 in paced mixed datagrams (about 54%). Private
routing changed less. Counts demonstrate the protocol effect, not CPU shares.
Small ACK tails before measured-gate capture are included; per-connection I/O
counters are not kernel syscall counts.

All 18 short and 18 longer reliable cases passed with fixed clients. Short medians
changed -1.61% private, +6.37% room, -0.26% mixed. The separate twelve-second
cohort measured 1,423,648/1,418,769 private (-0.34%), 1,333,525/1,354,868 room
(+1.60%), and 1,258,882/1,340,307 mixed (+6.47%). Variation is substantial; no
uniform throughput win or native WebSocket parity is claimed. Both cohorts and
all samples remain in the report.

At 100,000 sessions, each ordinary resource case completed 400,000 checked
deliveries and clean retirement. Server CPU was 7.96/7.10 seconds, client CPU
7.99/7.06 seconds, and ready RSS 2,482.79/2,473.41 MiB. This is one matched pair,
not a confidence interval. All five replacement-allocator gates completed;
ready requested live bytes were 2,420,719,873. Allocator-diagnostic CPU/latency/RSS
are excluded from ordinary comparisons; kernel costs remain separate incomplete
accounting. Full memory parity remains far away.

All six high-paced reliable cases passed with zero unreceived application
messages. All six paired datagram cases passed integrity/cleanup, but candidate
runs had private/fanout missing counts(4470,11620),(4278,17626),(3377,14162),
with 8669,10823,9001 client UDP drops. Baseline counts were(0,0),(5495,15310),
(0,0), with 0,8452,0 client drops. Server UDP receive drops were zero. This is a
material stress-test tradeoff; do not call the policy universally better or
loss-free. Causes of individual client drops are not inferred from these
counters alone.

Both endpoint profiles had zero lost samples. All nine clean/loss/reorder
fixtures passed. Under loss plus bulk traffic, datagram p99 state age was 70.35 ms
with 14/360 missing, versus 1,206.84 ms stream and 1,181.38 ms TLS WebSocket with none
missing. Datagram bulk arrival was 1,056.95 ms. Under reordering, datagram p99 age
was 99.98 ms with 5 missing and 7,219.34 ms bulk arrival. These single cases show the
freshness/bulk tradeoff; they are not statistical rankings. Chromium passed all
nine actual checks, including 5 MiB delivery and cleanup. All 274 relay tests and
Clippy passed. Initial invalid configuration setup artifacts are preserved and
excluded; the corrected CLI/test factory wiring is covered by the final runs.

Retain for reduced protocol traffic and CPU, and the observed longer mixed
throughput gain, with the datagram-pressure tradeoff explicit. Selected relay
SHA256 is `6d24b6bd04f3a114694560b846f5681b73dba765c82a8c951721743d37c69a8f`.
No push, deployment or PR occurred. Full throughput and memory parity remain
unachieved, and the pivot is not accepted.

### Heap unwinding attribution — 2026-10-21

The [unwinding verification](reports/webtransport-heap-unwinding-2026-10-21.json)
addresses truncated caller evidence in the original replacement-allocator
profiles. The maintained collector can explicitly select a supported stacktrace
method and verifies the profiler's real startup confirmation. The analyzer adds
optional grouping by all captured executable frames; its default site rows
match the existing 100,000-session profile exactly. It does not recover omitted,
inlined or shared-library frames.

A separate 8,192-session `libunwind` diagnostic passed all five gates and 32,768
measured checked deliveries. Ready buckets sum to 203,079,200 requested bytes
with no unresolved allocations. Unlike the truncated profiles, useful callers
now identify the event-queue deque, stream assembler heap, TLS handshake buffer,
and other vectors. An 832-byte event-queue allocation at executable offset
`0x24ca97` matches the same site's 99,965 objects / 83,170,880 bytes in the
original full-scale profile. No reclamation policy or product change is inferred
from that fact alone. The full 100,000-session libunwind capture is running;
its diagnostic watchdog is extended without changing protocol deadlines.

The initial full-scale `libunwind` run later failed at the connected gate:
`war_connections` was28,634 instead of100,000, with no valid captured gates.
An earlier read-only observation saw93,926 joins but31,409 live sessions, without
handshake or protocol errors. The slow profiler made setup take several minutes;
the retained client transport source defaults to30s QUIC idle timing. Retirement
is consistent with that timing, but individual close reasons were not captured.
This failed run is preserved and is not a full-scale memory baseline.

A separate full-scale diagnostic used the existing five-second server
keepalive option and the same frozen binaries. Its source receipt records that
wire-maintenance traffic differs from the ordinary workload; no product code or
default changed. All timing and RSS remain excluded from ordinary comparisons.
The maintained collector did not need a new keepalive option: capacity already
supports `ping_interval_secs` in its source arguments. An initial convenience
hook failed before server startup and is also preserved; it was removed. The
diagnostic reached 100,000 live sessions at connected and ready, then failed
the traffic population check at 97,525 sessions. Its ready snapshot supports
limited caller attribution only; it did not pass the complete benchmark.

### Completed handshake input-buffer candidate

The partial ready stack capture attributed 36,083,460 requested bytes to
99,515 TLS handshake input vectors. A candidate now releases a consumed input
vector after handshake completion only when no handshake fragments remain.
The ordering and progress state is preserved. The first fixed-client cohort
passed all 18 reliable cases; medians changed -0.58% private, +1.91% room, and
+1.03% mixed, which does not establish a throughput win. Both ordinary 100k
runs checked 400,000 deliveries. Candidate ready RSS was 2,435.41 MiB versus
2,473.09 MiB for the baseline. A separate candidate allocation run passed all
five gates with 2,380,842,704 requested live bytes at ready and zero unresolved
bytes in the exact-binary site analysis. Longer traffic, datagram, loss, endpoint
profile, and browser acceptance remain pending; the candidate is not yet
accepted. Memory and throughput parity remain unproven. See
`reports/webtransport-handshake-buffer-2026-10-22.json` for the initial receipts.

The longer 12-second cohort also passed all 18 cases, with candidate medians
+0.48% private, -2.29% room, and -3.05% mixed. These results reverse the small
room/mixed gains in the initial cohort. The throughput cost remains unresolved;
the candidate is still unaccepted. Paced traffic and both-endpoint profiles
are the next checks.

The candidate subsequently passed six paced reliable cases with zero missing
deliveries, six paired datagram cases, nine isolated freshness cases, and nine
actual Chrome checks including a byte-exact 5 MiB message. Both endpoint CPU
profiles reported zero lost samples. Datagram loss varied across repetitions;
no consistent datagram performance improvement is claimed. Under 2% loss, the
candidate datagram case had p99 freshest-state age 72.68 ms with 7/360 missing
updates, versus 1,281.24 ms for reliable QUIC and 1,141.48 ms for the TLS
WebSocket fixture (both zero missing). These are individual cases.

Browser cleanup reads showed all current session and resource gauges at zero.
Two local assertion mistakes (wrong metric name, then inclusion of historical
peak) are recorded in the report rather than treated as successful assertions.
A second 12-second cohort with reversed variant order is running to resolve
the earlier room/mixed throughput regression before retaining the change.

The reversed-order confirmation passed all 18 cases: +0.69% private, -0.94%
room, -0.24% mixed median throughput. The buffer reclamation is retained as a
verified memory reduction with explicit throughput uncertainty; both longer
cohorts had lower room/mixed medians and no throughput win is claimed. A
second Chrome run passed all nine checks and the corrected cleanup assertions
passed with exact current metric names. The complete evidence and earlier
script errors remain in the report. Overall native WebSocket throughput and
memory parity are still not achieved.

### Rejected event-overflow thread cache

The event queue retained roughly 83 MB of empty overflow storage at 100k
sessions. A per-thread spare experiment reduced ordinary ready RSS by
78.04 MiB and requested live memory by 82.74 MB in separate captures. It
passed both reliable cohorts and full capacity, but one paced private case
closed 15 slow consumers; four subsequent controls per variant passed, so
that failure cause remains unproven. Initial throughput medians changed
-1.09% private, +0.15% room, -1.65% mixed; longer medians +0.56%, -2.03%,
-0.68%, respectively.

A diagnostic established the actual design flaw: ingress producers requested
399,888 private-case and 110,447 mixed-case buffers with zero cache hits.
Connection drivers returned buffers on other runtime threads. The cache
therefore did not reuse overflow storage across those owners. The experiment
was rejected and its source/patch archived; retained product source and
release executable were restored. A cache spanning both owners is a distinct
future experiment. See `reports/webtransport-event-spare-2026-10-22.json`.

### Retained endpoint receive-batch event buffer pool

The endpoint-owned pool supersedes the rejected thread-local cache and the
unretained CPU-sized pool. It caches empty small overflow buffers across
ingress and connection-driver runtimes, sized to one UDP receive batch.
Diagnostic cache reuse reached 99.99% private and 99.73% mixed, with zero
full-cache returns. Queue depth, delivered events, FIFO and scheduling budgets
are unchanged. Both 100k ordinary runs checked 400,000 deliveries and cleanup;
ready RSS fell from 2,437.88 to 2,356.22 MiB. The separate allocation diagnostic
passed all five gates with 2,299,061,739 requested live bytes at ready.

Initial private/room/mixed median throughput changed +2.21/+1.48/-0.45%; longer
medians -0.24/-1.49/+2.17%. Both cohorts passed all 18 cases. Six paced reliable
cases had zero missing deliveries; six paired datagram cases passed but loss
varied, including one candidate run with 13,042 private and 43,852 fanout
updates unreceived. Both endpoint profiles reported zero lost samples; all
nine freshness and nine actual Chrome checks passed, including a byte-exact
5 MiB application message and zero current cleanup gauges. The selected change
is a verified memory improvement, not a universal throughput or datagram win.
See `reports/webtransport-event-batch-2026-10-22.json`. Overall native WebSocket
throughput and memory parity remain unmet.

### Retained final packet-space retirement

Initial and Handshake spaces now release their separately owned storage after
final key discard; Data stays inline and its live history is preserved. Retired
spaces cannot be recreated by lookup. Retry is explicitly separate: it replaces
Initial keys in live storage while preserving the next packet number. This
reduced protocol Connection size by 1,072 bytes, and actual Quinn connection
allocation from 6,416 to 5,344 bytes. Both 100k ordinary runs verified 400,000
deliveries and cleanup; ready RSS fell from 2,357.13 to 2,256.82 MiB.

Initial private/room/mixed median throughput changed -0.43/+1.77/-2.05%; longer
medians -0.08/+2.04/-1.10%. Mixed private and fanout components both declined
slightly, without a large hidden composition shift. Both cohorts passed all 18
cases. All six paced reliable cases had zero missing deliveries. Three paired
datagram candidate runs had fewer unreceived updates than their baselines, but
no general reliability win is claimed. Both endpoint profiles had zero lost
samples, all nine freshness cases passed, and all nine Chrome checks passed
with zero current cleanup gauges. The relay suite, Clippy and 319 protocol
tests passed, including terminal-retirement and late-packet replay checks.

The completed allocation capture measured 2,192,201,779 requested live bytes
at ready with no unresolved bytes and all five gates passed. This memory
improvement retains an explicit mixed-throughput cost and does not establish
overall native WebSocket parity. See
`reports/webtransport-packet-spaces-2026-10-22.json`.

### Fresh retained native comparison at 455aa94

The same retained executable and fixed inspection/capacity clients were used
for plaintext native WebSocket and encrypted QUIC/WebTransport. Three rotated
12-second runs per private/room/mixed workload passed, as did both 100k-session
capacity runs with 400,000 checked deliveries. Median verified rates were:

| Workload | Native WS deliveries/s | WebTransport deliveries/s | WT change |
| --- | ---: | ---: | ---: |
| Private | 1,198,539 | 1,410,552 | +17.69% |
| Room | 1,513,939 | 1,444,326 | -4.60% |
| Mixed | 1,505,839 | 1,383,842 | -8.10% |

Private parity is exceeded in this cohort; room/mixed parity and memory parity
remain unmet. The ordinary capacity pair measured ready RSS 112.69 MiB native
and 2,254.89 MiB WT, server CPU 2.00/6.86 seconds and client CPU 2.47/7.12
seconds. P99 round-trip histogram upper bounds were 1.05/2.10 ms. This is one
capacity pair; timings are not a repeated estimate. Kernel CPU work outside the
profiled processes is not comprehensively attributed.

Kernel accounting remains separate. Ready snapshots found 100k server TCP
sockets versus one shared UDP socket. A read-only root slab-size query on the
same running kernel supplied TCP=2,432 and UDP=1,344 bytes, giving a partial
TCP object estimate of 231.93 MiB versus 1,344 bytes UDP. Original unprivileged
size reads were unavailable, not zero memory. These estimates exclude other
kernel costs. Socket buffer limits are not allocated bytes. Native retirement
left 100k port-matched TIME_WAIT entries outside process RSS.

Ordinary done-gate RSS was 58.26 MiB native and 2,261.24 MiB WT. A separate
normal-glibc observer, with no replacement allocator, trim or tunables, found
WT RSS 2,259.21 MiB five seconds after retirement: 2,294,516,240 bytes free in
allocator arenas, 11,654,640 bytes used in arenas and 60,985,456 bytes in
allocated mmap regions, with zero current session/resource gauges. XML
statistics covered all eleven arenas. Thus the large retired RSS plateau is
mostly allocator-retained free storage, not multi-gigabyte live sessions. This
does not resolve the genuine connected-population footprint. The observer
adds a diagnostic thread and its timing/RSS is excluded from ordinary results.

Matched-rate cycle profiles of both endpoints reported zero lost samples.
Native server cycles were 83.39% kernel and 15.36% executable; WT server cycles
were 19.47% kernel and 70.62% executable. Driver polling, task wakeups, event
dispatch, notification polls and timer resets are prominent WT symbols. These
are sampled cycles, not exact subsystem elapsed-time attribution. The next
CPU investigation should target the integration/wakeup path; overall parity
is still unmet. See `reports/webtransport-retained-parity-2026-10-22.json` for
all component rates, phases, kernel observations, profiles, observer failures
and reproduction sources.

The cumulative allocation census strengthens this diagnosis. Both captures
used the same retained executable and fixed clients, all 100k sessions and
400k checked deliveries, with bucket/header totals and exact ELF identities
verified. Native requested 833 allocations (0.00208/delivery) and WT 3,630,150
(9.075/delivery). WT requested 3,626.58 allocation bytes per checked delivery
versus native 2.96; these are allocator requests, not copy bytes, and include
background work in the gated interval. Packet tracking accounted for 419,098
requests / 486,153,680 bytes, endpoint receive storage 799,885 / 447,594,116,
and application record buffers 400,000 / 414,800,000. The diagnostic allocator
can alter batching, so these counts explain sites rather than ordinary timing.
Exact-build glibc debug symbols also identify prominent allocator/free samples;
missing caller frames prevent assigning those CPU samples to a Rust call site.
The next experiment should target shared packet-tree reuse, with actual reuse
and allocation measurement before acceptance. The earlier one-per-worker tree
cache remains rejected; it is not evidence that a shared cache will win.


### Shared packet-tree pool rejected (2026-10-22)

The endpoint-owned empty-tree pool reduced ready-to-measured allocator
requests from 9.075 to 8.030 per checked delivery and requested bytes from
3,626.58 to 2,416.30, with 100,000 sessions and 400,000 verified deliveries.
Packet records and scalar connection history stayed with their connection;
the protocol suite passed 320 tests with paired-endpoint pooling disabled
and enabled. The relay suite and Clippy passed.

That allocation improvement did not establish a whole-workload win. Three
rotated four-second repeats gave median throughput changes of -0.43% private,
+0.26% room, and -1.33% mixed. The ordinary capacity pair passed, with ready
RSS 2,254.73 versus 2,253.89 MiB and server CPU 6.72 versus 7.10 seconds.
Small timing differences and this single capacity pair do not prove a
regression, but neither justifies the shared-lock complexity. Synchronization
is a hypothesis, not a demonstrated cause. The candidate was rejected and
source plus default executable restored to the retained packet-space version.
Browser and datagram acceptance were not run for the rejected candidate.

The exact rejected diff, hashes, receipts, allocation sites, validation, and
measurement limits are preserved in
[`webtransport-shared-packet-tree-2026-10-22.json`](reports/webtransport-shared-packet-tree-2026-10-22.json).
UDP receive storage is the next allocation target; throughput and memory
parity remain unmet.


### Endpoint-local UDP receive storage rejected (2026-10-22)

A FIFO of empty BytesMut tails, bounded by existing receive byte/segment
geometry, safely reused backing storage using try_reclaim. Packet slices
retained normal ownership; outstanding references forced fresh allocation.
The cache had no shared mutex. Focused ownership/reuse checks, relay tests,
Clippy, and both 100k capacity runs passed.

The diagnostic allocation census fell from 9.075 to 5.073 requests per
verified delivery and 3,626.58 to 2,418.11 requested bytes per delivery.
Receive-site allocations fell from 799,885 to 2,319, and split_to requests
from 799,903 to 2,336. These are allocator requests under the diagnostic
allocator, not copied bytes or ordinary performance results.

Three four-second repeats initially gave +1.36% private, +2.80% room,
-0.48% mixed. Three rotated twelve-second repeats instead gave +0.63%
private, +0.91% room, and -5.46% mixed. The ordinary capacity pair added
2.94 MiB ready RSS; server CPU was 6.70 versus 6.75 seconds for 400k verified
deliveries. Reduced allocations did not translate into a whole-workload win.
The cause of the mixed regression remains unproven. Reject this design;
source and default executable were restored. Browser/datagram/freshness
acceptance was not run after the performance selection gate failed.

Exact source, hashes, receipts, longer distributions, and limits are in
[`webtransport-receive-storage-2026-10-22.json`](reports/webtransport-receive-storage-2026-10-22.json).
Overall throughput and memory parity remain unmet.


### Send-first UDP writability retained (2026-10-22)

Connections now try the nonblocking send before creating or polling a
writability notifier. WouldBlock registers the notifier before yielding;
the exact transmit bytes remain retained until retry. Rebinding discards
the old socket's notifier. A stable-server/rebinding-client test forces
WouldBlock and Pending, then verifies 64 KiB through each of two rebinds.
The original self-connected rebind fixture failed on both unchanged control
and candidate; that diagnostic failure is preserved rather than attributed
to the optimization.

The ordinary 100k pair saved 15.42 MiB ready RSS (2,252.68 to 2,237.26 MiB),
with 400k verified deliveries each. Server CPU was 6.83/6.75 seconds, p99 RTT
histogram upper bound 2.097152 ms in both; one pair is not a confidence
interval. Ready requested-live heap fell 2,192,201,779 to 2,173,376,411 bytes
(17.95 MiB). The writability-poller site disappeared from ready attribution.

Short-cohort throughput medians fell 0.98% private, 1.90% room, 5.29% mixed.
The separate three-repeat twelve-second cohort instead measured -1.21%
private, +2.62% room, +2.30% mixed. Retain this memory and room/mixed gain
with its explicit private cost and shared-host variability, not as a
universal speedup. Both-endpoint before/after profiles had zero lost samples.
Six paced reliable controls passed. Six paired datagram controls passed;
candidate missing updates improved in two pairs and worsened in one.

All nine impaired-network freshness receipts passed, as did all nine Chrome
checks including exact 5 MiB delivery. Connection/session/QUIC, queued-byte,
variable-byte and read-budget gauges returned to zero. Relay tests, Clippy,
the Quinn fixture suite (30 passed, 3 existing ignored), baseline blocked-send
control and pinned vendor verification passed. Browser and fixture closed.
The full source, reproduction scripts, fixed-client hashes, distributions,
measurements, fixture failure and scope limits are in
[`webtransport-lazy-writable-2026-10-22.json`](reports/webtransport-lazy-writable-2026-10-22.json).
Overall throughput and memory parity remain unmet.


### Temporary QUIC secret handoff storage boxed (2026-10-22)

Rustls's QUIC state retained inline Option<Secrets> storage for handshake
and application key handoffs even after write_hs took those values. Boxing
only those two temporary values preserves the original handoff/derivation
points. Early-data secret state, packet/header cryptography and precomputed
next packet keys are unchanged. This introduces temporary handshake
allocations, rather than per-packet allocations.

Compiler layout: QUIC server 1,160 to 824 bytes. Actual 100k TLS-session
allocations: 1,368 to 1,032 bytes each. Ordinary ready RSS fell 2,236.04 to
2,205.41 MiB (-30.63 MiB). Total requested live allocation fell 2,173,376,411
to 2,142,329,135 bytes (-29.61 MiB); the total includes other live sites.
Both 100k runs checked 400k deliveries. Setup was 11.47/11.40 seconds and
server CPU 6.83/6.65 seconds; a single pair is not a confidence interval.

Three short repeats measured -0.49% private, +4.43% room, -2.43% mixed.
Three twelve-second repeats measured +0.98%, +1.54%, -0.58% respectively.
Retain the measured memory saving with the explicit mixed-throughput cost;
this does not establish a universal throughput improvement or full parity.

Relay tests and Clippy passed. Selected Rustls suite: 246 passed; selected
Quinn protocol suite: 312 passed, including key-update/reordering and
0-RTT acceptance/rejection. Six paced reliable runs had zero missing,
duplicate or reordered deliveries. Six paired datagram controls passed,
with nonuniform loss. Both-endpoint before/after profiles had zero lost
samples. All nine freshness receipts and nine Chrome checks passed,
including exact 5 MiB delivery. Cleanup gauges were zero, fixtures stopped,
and the pinned vendor patch verified. Full evidence, source, fixed-client
hashes, reproduction scripts and limits are in
[`webtransport-boxed-quic-secrets-2026-10-22.json`](reports/webtransport-boxed-quic-secrets-2026-10-22.json).

A separate next target is the drained handshake-span vector's preallocated
storage. It is not changed or counted as a saving here. Overall throughput
and memory parity remain unmet.


### Empty QUIC handshake-span storage released (2026-10-22)

The completed-QUIC-handshake, inactive-deframer, empty-input guard now also
releases the empty fragment-span vector. It preserves outer_discard and
ordering state, leaves active/incomplete fragments untouched, and retains
initial handshake preallocation. TCP/TLS record processing is unchanged.
A focused test covers incomplete fragments, progress preservation, release,
and subsequent parsing. Rustls passes 247 tests; the selected Quinn protocol
suite passes 312. Relay tests, Clippy and pinned vendor verification pass.

The 100k pair checked 400k deliveries each. Ready RSS fell 2,204.26 to
2,141.37 MiB (-62.89 MiB); setup was 11.37/11.34 seconds, server CPU
6.59/6.75 seconds. These single-pair times are not confidence intervals.
Requested-live heap fell 2,142,329,135 to 2,076,389,859 bytes. The prior
100k x 640-byte constructor allocation site disappeared; TLS sessions
remain 100k x 1,032 bytes, separating this from the prior boxed-secret gain.

Three short repeats measured -0.87% private, -2.86% room, -4.72% mixed.
Three twelve-second repeats instead measured +0.93%, -0.07%, -0.05%.
Retain the memory saving with longer throughput essentially unchanged;
shared-host variability and the worse short cohort remain in the report.
Six paced reliable runs had zero missing/duplicate/out-of-order delivery.
Six paired datagram controls passed; candidate loss was worse in two pairs
and better in one. Both-endpoint profiles had zero lost samples. All nine
freshness cases and nine headed Chrome checks passed, including exact
5 MiB delivery. Cleanup gauges were zero and fixtures stopped.

Source, hashes, reproduction scripts, fixed-client workloads and evidence:
[`webtransport-handshake-spans-2026-10-22.json`](reports/webtransport-handshake-spans-2026-10-22.json).
Overall parity remains unmet; refresh native WebSocket comparisons next.


### Current native parity audit after 29ff41d (2026-10-22)

The same retained executable runs both native plaintext WebSocket and
encrypted WebTransport, with fixed clients and no concurrent task-owned
benchmarks. Three rotated twelve-second runs per workload measured native/WT
median verified deliveries/s: private 1,196,677/1,427,686 (+19.30% WT), room
1,553,659/1,434,141 (-7.69%), mixed 1,459,897/1,406,145 (-3.68%). Private meets
this throughput target; room and mixed do not. This is not acceptance of
overall parity.

The fresh 100k pair checked 400k deliveries each. Native/WT ready RSS was
114.32/2,142.35 MiB (18.74x); setup 0.668/11.613 seconds; server CPU
2.05/7.39 seconds. User CPU was 0.42/6.05 seconds and system CPU 1.63/1.34:
the large extra cost is in user-space processing. Both p99 RTT histogram
upper bounds were 2.097152 ms, with maxima 3.586/16.053 ms. Single-pair times
are not confidence intervals. Client ready RSS was 613.57/4,274.29 MiB and
client CPU 2.42/8.42 seconds; clients stayed fixed. Both application resource
gauge sets reached zero. Done server RSS was 58.28/1,939.35 MiB; high RSS
alone does not establish a live allocation leak.

Current-executable diagnostic heap captures passed all five gates and
400k deliveries: native ready live 118,841,155 bytes, WT 2,076,389,859 bytes
(17.47x). WT used the immediately pre-commit capture of the identical
executable and traffic workload. Diagnostic allocator counts are separate
from ordinary RSS/timing. Kernel accounting found 100k live TCP sockets
versus one UDP socket; partial socket-object estimates 243,200,000 versus
1,344 bytes. Buffer limits are not allocated bytes, skmem counters are not
an additive total, and TCP TIME_WAIT objects are reported separately.

Matched-offered-rate both-endpoint profiles had zero lost samples. Server
self samples: native 78.71% kernel/20.90% executable; WT 20.12% kernel/71.05%
executable/7.44% libc. WT connection-driver polling, task wakeups, event
queue dispatch, notifications and timer reset remain prominent. These
support a worker-grouped driver scheduling experiment while preserving
independent UDP ingress; they do not establish that any particular pooling
implementation will win. This differs from the rejected app/driver
co-driving prototype and requires new whole-workload evidence.

Full distributions, all component measurements, capacity phases, client
metrics, kernel counters, exact hashes, profiles, heap attribution and
reproduction sources are in
[`webtransport-retained-parity-29ff41d-2026-10-22.json`](reports/webtransport-retained-parity-29ff41d-2026-10-22.json).
Datagram/freshness/browser acceptance for this binary remains in the prior
handshake-span report. Overall throughput and memory parity remain unmet.


### Grouped connection drivers rejected (2026-10-22)

An explicit Runtime::spawn_connection hook grouped opaque Quinn connection
futures on one FuturesUnordered worker per actual application-runtime worker.
UDP ingress remained independent; child wake registrations and the existing
protocol engine remained intact. Poll/destructor panic isolation, parked
child cancellation and synchronous bind fallback were checked. Initial
synchronous bind panicked outside Tokio; the corrected fallback passed the
full relay suite and Clippy. This differs from the prior app/driver co-driving
experiment, but still did not establish an improvement.

Three rotated four-second throughput repeats measured -7.70% private,
-9.18% room, -3.20% mixed. Every private/room candidate sample was below every
control sample. A separate private diagnostic reproduced lower throughput.
All 100k capacity checks and 400k deliveries passed. Ordinary ready RSS saved
6.19 MiB (2,142.45 to 2,136.26 MiB); CPU 6.82/6.80 seconds and setup
12.21/11.27 seconds are one pair, not confidence intervals.

All five heap gates passed. Requested-live bytes fell 2,076,389,859 to
2,071,958,700 (4.23 MiB). The ordinary driver allocation site changed from
100k x 128 bytes to 100k x 88 bytes, saving 40 requested bytes per connection.
That is much smaller than the separate application task and protocol states.
Eight endpoint symbol profiles across private/mixed, baseline/candidate,
server/client had zero lost samples. FuturesUnordered introduced its own
ArcWake work, but the complete cause of throughput loss is not established.

Reject the prototype; original source and selected executable restored.
Longer throughput, browser/datagram/freshness checks were not run after the
performance selection gate failed. Exact source, failure/correction,
reproduction scripts, fixed clients, profiles and measurements are preserved
in [`webtransport-grouped-drivers-2026-10-22.json`](reports/webtransport-grouped-drivers-2026-10-22.json).
Overall throughput and memory parity remain unmet.


### Contiguous stream ACK prefix fast path retained (2026-10-22)

When a clamped stream ACK begins at the first buffered byte and no
out-of-order ACK ranges are pending, SendBuffer now discards that exact
prefix directly. The original range-tree path handles other cases. A shared
prefix-discard helper preserves segment ownership and front-trimmed accounting.
No range-set type, ACK policy, packet-number policy or resource budget changes.
Focused tests cover partial segments, overlapping reordered ACKs, and old
duplicates while out-of-order ranges are pending.

Both 100k capacity runs verified 400k deliveries. Ready RSS fell 2,142.12 to
2,082.50 MiB (-59.62 MiB); setup 11.54/11.36 seconds and server CPU 6.61/6.61
seconds are a single pair. Requested live bytes fell 2,076,389,859 to
2,018,187,005 (-55.51 MiB), with all five heap gates passing. The prior
300k x 192-byte range-tree insertion site disappeared. Fragmented ACKs
retain the original fallback and may still allocate trees.

Three short repeats measured +1.10% private, +1.74% room, +3.42% mixed.
Three twelve-second repeats measured +0.14%, +0.83%, -1.14%. Retain the
measured memory saving with the explicit mixed-throughput cost and timing
variability; do not use the larger short gains as the final speed claim.
Relay tests, Clippy, 314 protocol tests and pinned vendor verification pass.
Six paced reliable controls had zero missing/duplicate/out-of-order messages;
six paired datagram controls passed with variable loss. Both-endpoint
profiles had zero lost samples. All nine freshness receipts and nine Chrome
checks passed, including exact 5 MiB delivery. Cleanup gauges returned to
zero, browser and fixtures stopped.

Exact source, fixed clients, distributions, component metrics, heap captures,
profiles and reproduction scripts are in
[`webtransport-ordered-ack-2026-10-22.json`](reports/webtransport-ordered-ack-2026-10-22.json).
Full throughput and memory parity remain unmet.

## Receive cache geometry experiment — rejected (2026-10-22)

Compared endpoint-local exact-size receive-storage caches with 32 and 2,048
slots against retained ordered-ACK, with fixed clients and rotated runs.
Three twelve-second repeats measured small-cache private +0.63%, room
-0.97%, mixed +0.70%; large-cache +1.04%, +0.42%, -2.31%. The short
small-cache mixed +4.00% gain did not persist. All 27 longer receipts passed.
100k-session ready RSS was 2,081.34 / 2,080.47 / 2,083.22 MiB for
baseline / small / large: no meaningful memory improvement. All three
capacity runs verified 400k measured deliveries.

Diagnostic cumulative allocation requests per delivery fell 9.096 to 8.361
and 7.125, but allocation savings did not establish a whole-workload win.
Instrumented private/mixed miss counts showed only 3.7–4.9% reuse for
32 slots and 24–35% for 2,048. Size mismatch dominated the large cache;
small-cache storage was also often not reclaimable. These counters do not
prove a cache-locality cause. Four diagnostic receipts passed.

Quinn fixture tests passed (33, three ignored). The final root suite had a
failed embedding-service cancellation test (64 versus 63); preserve that
failure and do not claim full-suite success. Broad acceptance gates were
not run because both candidates failed selection. Both source variants
were removed and the exact retained ordered-ACK executable restored
(SHA256 8e11a5d0ffa1d94c2da55cfa9ada4df23cffdbb4d14b8a5832af7648892ac99a).

Source, immutable receipts, distributions, allocation diagnostics, census,
validation limits and reproduction scripts:
[`webtransport-receive-cache-geometry-2026-10-22.json`](reports/webtransport-receive-cache-geometry-2026-10-22.json).
Throughput and memory parity remain unmet; the overall goal remains active.

## Completed QUIC exporter state — rejected (2026-10-22)

Specialized the completed server QUIC TLS state to retain exporter material
instead of the full TLS traffic schedule; TCP TLS and QUIC packet-key
state were unchanged. Candidate built; 249 copied Rustls unit tests passed,
including SHA-256/SHA-384 exporter equivalence with varying contexts and
lengths. The public API QUIC exporter test passed after a real handshake.
Original fixture compilation errors and corrected successful logs remain.

All 18 short and 18 longer fixed-client receipts passed, as did both
100k capacity runs. Longer medians regressed private 1.22%, room 5.09%,
mixed 0.66%. Ready requested live bytes fell from 2,018,187,005 to
2,003,818,662 (-13.70 MiB), consistent with 144 bytes removed per server
TLS state. Ordinary ready RSS also fell roughly 15 MiB. Allocation
diagnostics do not explain the throughput regression; do not assert an
instruction/cache-layout cause without measurement.

Rejected: the small memory saving does not justify the observed throughput
cost. Source and exact retained ordered-ACK executable restored. No broad
acceptance for this rejected candidate. Full parity remains unmet.
Evidence and reproduction:
[`webtransport-quic-exporter-rejected-2026-10-22.json`](reports/webtransport-quic-exporter-rejected-2026-10-22.json).

## Compact connection RNG — rejected (recorded 2026-11-06)

Completed Oct 22 measurements used a 48-byte seed/word-position state in
place of the buffered 320-byte connection StdRng, reconstructing the same
pinned ChaCha12 generator on random requests. Initial constructor calls
remained buffered. Mixed-call byte/u32/u64 sequence equivalence passed,
as did 322 copied Quinn protocol tests, 18 short and 18 longer throughput
receipts, and both 100k capacity runs (400k measured deliveries each).

Longer medians regressed private 2.09%, room 3.64%, while mixed rose 0.78%.
Requested live bytes fell 2,018,187,005 to 1,990,972,072 (-25.95 MiB);
main connection allocation shrank 272 bytes. Ordinary ready RSS fell
2,083.65 to 2,053.98 MiB (-29.67 MiB). The memory reduction is real, but
it does not justify the observed private/room cost. Reconstruction cost
is a hypothesis rather than a profiled causal finding.

Rejected; original source, lockfile and exact retained ordered-ACK binary
restored. Broad acceptance gates intentionally not run for the rejected
candidate. Throughput and memory parity remain unmet.
Evidence and reproduction:
[`webtransport-compact-rng-rejected-2026-11-06.json`](reports/webtransport-compact-rng-rejected-2026-11-06.json).

## Initial expanded key-update pair — rejected (2026-11-06)

Deferred the first expanded 1-RTT successor pair until a local update or
Short-header phase mismatch, then kept original eager-successor behavior.
Cached preparation survives failed authentication without advancing secrets
again. Tests cover idle absence, local/remote update, reordering, packet
capture and repeated corrupted updates followed by valid data. All 322
copied protocol tests and 253 relay tests passed (one existing ignored);
Clippy and pinned-vendor verification passed.

All 18 short and 18 longer fixed-client throughput receipts passed. Longer
medians: private -1.05%, room -1.70%, mixed +1.03%. Both 100k capacity
runs passed with 400k measured deliveries. Ready RSS fell 2,081.14 to
1,962.60 MiB (-118.54 MiB). Fresh paired allocator captures passed all
five gates: requested live bytes fell 2,018,178,503 to 1,902,036,045
(-110.76 MiB), packet-key allocations 400,040 to 202,364. Savings apply
to sessions before their first update; later behavior resumes eager preparation.

Rejected on reliable-load acceptance: initial candidate paced runs passed
5/6, with mixed run 3 closing slow consumers (1013). Fresh rotated mixed
controls passed baseline 3/3 versus candidate 2/3; the candidate failed
again with slow-consumer closures. Six paired datagram controls passed
with loss recorded separately. No resource budgets were raised.

Kernel SOCK_DIAG returned ENOENT for both protocols/families. Fresh paired
allocation-only receipts explicitly mark kernel accounting unavailable, never
zero; the failed original capture is retained. Privileged profiling failed
under no-new-privileges. User-only capture initially exceeded mmap-lock
allowance at 512 pages; fresh 64-page captures succeeded for both endpoints
with zero lost samples. Both profiled workloads passed, so those profiles do
not explain the intermittent overload. Delayed derivation as cause remains
unproven. Browser/freshness gates were skipped for the rejected candidate.

Original source, provenance/patch and exact ordered-ACK executable restored.
Overall throughput/memory parity remains unmet. Full evidence, rejected
diff, failed trials, profile limits and reproduction scripts:
[`webtransport-first-key-update-rejected-2026-11-06.json`](reports/webtransport-first-key-update-rejected-2026-11-06.json).

## Retained ordered-ACK parity refresh — 2026-11-06

Same frozen server image (7d7eec6) and clients, rotated three12s runs:
QUIC medians private +17.88%, room +1.33%, mixed +10.23% versus native
plaintext WebSocket. Room distributions overlap; do not claim universal or
statistical superiority. All18 receipts passed. Fresh100k capacity pairs
passed400k measured deliveries each; ready RSS native111.99MiB versus
QUIC2083.02MiB. RTTp99 bucket upper bounds1.048576ms versus2.097152ms.
Memory parity remains far from met.

Both-endpoint mixed user-cycle leaf profiles passed with zero lost samples.
Kernel accounting/profiling unavailable; never substitute zero. The initial
DWARF attempt failed permissions; a successful native leaf capture's report
hook failed and was recovered from the same data, with PIDs verified from
thread-affinity metadata. No ordinary timing includes profiling.
Detailed counters, CPU/delivery, setup, retirement, latency, hashes and scripts:
[`webtransport-retained-parity-ordered-ack-2026-11-06.json`](reports/webtransport-retained-parity-ordered-ack-2026-11-06.json).

## Datagram parts coalescing — not retained (2026-11-06)

Verified an integration double-copy: relay allocates a CBOR-frame Vec, then
WebTransport allocates again to prepend the session ID. A pinned library
method coalesced session header/routing prefix/body in one allocation.
Eleven library tests,253 relay tests(one existing ignored), Clippy, release
build and300 vendored-file verification passed. Fixed-client18short and
6long datagram receipts passed integrity/cleanup; user-space profiles of
both endpoints had zero lost samples.

A4000-submission allocator diagnostic removed exactly4000 relay temporary
allocations totaling4,148,000 bytes; final4000 transmit buffers remained.
Total requests fell15043 to10006, but packet batching also changed; do not
attribute the entire difference to this change or claim copied-byte counters.
Room CPU median fell11.14 to10.71s at the same offered load. Private
CPU2.06 to2.04s. Mixed CPU/arrival results were not a convincing win:
short missing counts increased; longer receipt distributions overlap widely.

Fresh reliable mixed controls failed baseline1/3 and candidate2/3 with
slow-consumer closes. This method is not called on reliable output; those
failures expose an unstable common acceptance boundary, not proof of a
parts-method cause. Do not replace the failed receipts with passing retries.
Restore retained source/binary and diagnose queue saturation/scheduling
before selecting more variants. Browser/freshness gates were not run for
this unselected change. No per-session memory benefit claimed.
Full source, exact hashes, counters, failures and reproduction:
[`webtransport-datagram-parts-deferred-2026-11-06.json`](reports/webtransport-datagram-parts-deferred-2026-11-06.json).

## Retained reliable overload diagnostic — 2026-11-06

A failure-only instrumented build records the first queue refusal, without
logging every closing peer. The first trial passed; the second reproduced
the close. First sampled refusal: immediate/private path, queue open and
full at its64-message limit, writer_busy=true, RTT3.179ms, cwnd20,536B,
118lost packets/12congestion events and one historical STREAM_DATA_BLOCKED
frame. A writer already owned an unfinished record; this is not evidence
of a wholly unstarted writer. It does not yet distinguish current stream
credit/send-window/congestion blocking from delayed repolling.

Matched paced controls used the same frozen server/client images, offered
load and CPU placement. Native WebSocket passed3/3; retained QUIC passed
2/3, with139slow-consumer events in its failing run. This verifies that
mean-throughput medians are insufficient acceptance; sample counts do not
establish a statistical failure rate. No resource budgets were raised.
The logging patch is diagnostic-only and not retained in production source.
All processes stopped; retained executable restored.
Exact traces, counters, source patch, controls and scripts:
[`webtransport-reliable-overload-diagnostic-2026-11-06.json`](reports/webtransport-reliable-overload-diagnostic-2026-11-06.json).

## Current write gate, peer pressure, and receive clock — 2026-11-06

A read-only first-refusal snapshot identifies the immediate private-write
gate: peer stream credit was zero, while connection credit was ample and
8.87 MiB remained in the local send window. The stream was open, not stopped,
and its writer remained registered as blocked. Historical loss and flow-control
counters alone could not establish this; the new current-state probe does.

An owned-inode UDP timeline finds client drops beginning 307.7–312.3 ms
before the logged refusal. The last pre-refusal sample had 8,460 drops and
5.82 MB queued; the pre-refusal peak was 6.79 MB. Server drops remained zero.
The passing trial had no client drops and a 0.45 MB peak. Later peaks/final
drop counts include post-refusal traffic and must not be used as preceding
causal evidence. This narrows the failure to receiver-side pressure and
stream-credit progress, without proving its CPU/scheduling/burst mechanism.

A separate library defect is now proven: Quinn captured one receive time
before draining several socket batches. Concurrent sends could then be newer
than their received ACKs. A real probe captured a negative interval, and all
three paired old-code probes observed the predicate. The correction samples
time immediately after each successful receive. None of the three fixed
probes observed it. A deterministic stateless-reset-throttle regression fails
on old code and passes with the fix; all 31 Quinn fixture tests pass, with
three existing ignored. Each probe variant still failed one paced trial, so
this correction is not a claimed overload solution.

The uninstrumented candidate passes relay tests, Clippy, vendor verification,
all 18 short throughput receipts, and both 100k capacity runs. Short medians
were private -1.01%, room -0.06%, mixed -1.79%, with essentially unchanged RSS.
Longer performance, traffic and runtime acceptance remain in progress; the
product change is uncommitted. No queue or receive/send resource budgets changed.

Evidence:
- [Current write gate](reports/webtransport-current-write-gate-2026-11-06.json)
- [Peer UDP pressure](reports/webtransport-peer-udp-pressure-2026-11-06.json)
- [Receive-clock diagnosis](reports/webtransport-receive-clock-diagnosis-2026-11-06.json)

Full throughput, memory and reliable-load acceptance remain unproven.

## Receive-clock correction retained — 2026-11-06

The completed acceptance record retains the post-receive timestamp correction
as a correctness fix. The selected release image is
`e1ac652735f2c91cadf31d65588852f39cad09e347a3686c90a938240a4c56e3`.
It contains no diagnostic probes. The deterministic old-code regression,
31 Quinn fixture tests, 253 relay tests, Clippy and vendor verification pass.
All 36 ordinary short/long throughput receipts passed. Longer median changes
against ordered-ACK are private -0.12%, room +0.83%, mixed +1.28%; these small
differences support no statistical speed claim.

Both endpoint profiles have zero lost samples. Actual Chrome passes all nine
checks, including the 5 MiB payload and datagrams, and all tracked connection
and output resources return to zero. Six datagram controls pass. Paced reliable
controls still include one candidate failure out of three, versus three passing
baseline controls; the clock defect is not established as the overload cause.

A separate opaque user-space UDP proxy exercises 20 ms delay and independent
2% loss in each direction without changing host networking. All four runs pass
their protocol and proxy checks. Candidate datagram p99 freshest-state age is
199.84 ms with 14/360 updates missing, versus 4921.00 ms and no missing updates
for streams. Reliable bulk arrives at 9141.12 ms with datagram state traffic,
versus 4937.94 ms in stream mode. Baseline datagram p99 is 103.99 ms; one run
per mode/variant does not establish a tail-latency trend. This proxy is not a
kernel netem or native TCP equivalence experiment.

Current ordinary 100k runs use the same selected release image for both modes:
QUIC ready RSS is 2080.46 MiB, native WebSocket 115.66 MiB (17.99x). Client
setup is 10.7385 s versus 0.6606 s. Separately captured requested live allocation
totals are 2,018,165,276 B versus 118,838,339 B at ready, and 70,821,883 B versus
1,846,611 B at logical done. Diagnostic allocator RSS is excluded from ordinary
RSS comparisons. Current kernel socket-memory/slab attribution remains
unavailable; it is not reported as zero or inferred from older kernel sizes.

The final report also corrects earlier setup labels: client-reported setup and
controller connected-gate wall time are now distinct. Raw receipts are unchanged.
Full evidence, hashes, commands, distributions and limits:
[receive-clock validation](reports/webtransport-receive-clock-validation-2026-11-06.json).

The full goal remains active: solve the measured receiver-pressure/stream-credit
failure and the large memory gap before accepting the pivot.

## Inline assembler screen rejected — 2026-11-06

A one-chunk inline assembler removes 300,000 x 224 B staging roots at the
100k ready gate, while enlarging each relevant connection/receive object by
32 B. The paired replacement-allocator total falls from 2,018,467,661 B to
1,963,888,112 B: a net 52.05 MiB saving. Ordinary ready RSS falls 55.36 MiB.
All 323 protocol tests, 253 relay tests, Clippy, vendor verification and the
100k routing/cleanup checks pass. These are persistent storage savings, not
per-delivery allocation claims.

The full 18-run, 12-second rotated comparison passes correctness but median
throughput falls private 1.34%, room 2.06%, and mixed 1.73%. Paced controls pass
baseline 2/3 and candidate 3/3; the baseline failure remains recorded and these
small samples establish no failure-rate improvement. The repeatable throughput
cost across all workloads outweighs this modest memory saving. The cause of
that cost has not been attributed to a particular instruction or cache layout.

The candidate is rejected. Source is restored and rebuilt to the selected
receive-clock image `e1ac652735f2c91cadf31d65588852f39cad09e347a3686c90a938240a4c56e3`.
Candidate source, binary hashes, failures, commands and measurements remain in
[the complete screen report](reports/webtransport-inline-assembler-screen-2026-11-06.json).
Browser, freshness and profile gates were skipped for this rejected variant.

## Retained-clock UDP and CPU correlation — 2026-11-06

A temporary first-refusal image based on the retained clock correction repeats
the precise credit snapshot while collecting owned UDP state and client-thread
CPU counters. Two trials pass; the third fails and ends the diagnostic's
predeclared up-to-six-trial stopping rule. These samples do not estimate a
failure rate. The fixed client and offered traffic remain unchanged.

The failing connection has a positive 13.95 us minimum RTT, peer stream credit
zero, 8,894,408 B of local send-window headroom, and a registered writer. Its
in-flight bytes are 18,876 against a 20,314 B congestion window; the remaining
1,438 B is below the current 1,452 B MTU. This is consistent with congestion
limiting normal packet drainage in addition to the immediate stream-credit
admission gate; the diagnostic did not observe a packetizer branch directly.

Client UDP drops are first observed 742.6 ms before logged refusal. The last preceding
sample has 12,301 drops and 6,429,240 B queued. A passing trial also has 7,571
drops, so drops alone are not sufficient for a close. Later peaks and totals
include post-refusal traffic and remain separate in the report.

Across actual sampled adjacent windows, aggregate client CPU increases from
4.956 to 5.493 core equivalents. All eight workers are balanced during the
drop-to-refusal window (0.679-0.708 each). This does not show sustained saturation
of all eight cores or identify one fixed hot thread. A migrating endpoint
future can still serialize work, and interval averages do not resolve bursts.
Kernel scheduler statistics are disabled; their raw wait fields cannot prove
absence of OS wait or quantify Tokio future queueing.

The diagnostic source is removed and the pure selected image is rebuilt and
verified as `e1ac652735f2c91cadf31d65588852f39cad09e347a3686c90a938240a4c56e3`.
No workload remains running. Exact timelines, window endpoints, counter checks,
observer cost, source and binary identities:
[UDP/CPU correlation report](reports/webtransport-peer-udp-cpu-correlation-2026-11-06.json).

Remaining causal work is to distinguish ingress/future fairness, catch-up
bursts, and loss/flow-control recovery. Throughput and memory parity remain
unmet; the compact established-session IO prototype is an unverified next
memory candidate, not a retained implementation.

## Compact established-session IO retained — 2026-11-06

The relay now consumes the generic WebTransport Session into established IO
and temporary stream acceptance. Once its application stream and join record
are admitted, empty acceptance machinery and unused request/open-operation
metadata are released. Partial extra streams, QPACK state and state shared with
legacy Session clones remain owned through close. Settings, CONNECT capsules,
session-ID framing and error mapping remain intact. Busy raw operations and
HTTP/3 poll-accept registrations return the original handle unchanged.

The pinned `web-transport-quinn` 0.13.3 archive is newly vendored with one
three-file patch. Root tests pass 253/1 existing ignored; all 28 library tests
and root/library Clippy pass. Focused real-QUIC tests verify wake registrations,
nonzero/raw IDs, empty-state release, partial/QPACK retention, clones and close
capsules. The caller retains acceptance state before propagating join errors
or timeouts. Full archive/provenance verification passes (300 files, six archives,
five declared patches).

Fresh matched 100k measurements reduce ordinary ready RSS from 2081.61 to
1889.27 MiB. Separate requested live allocation totals fall from 2,018,321,529 B
to 1,848,987,316 B, saving 161.49 MiB net. These totals include the relay task's
increase from 1920 to 2048 B per session; the Quinn core stays 5344 B. Setup in
the ordinary pair is 10.2477 versus 10.7656 s, and measured server CPU is 5.76
versus 5.96 s. These single-pair values are preserved, not presented as gains.

All 18 ordinary 12-second comparisons and all six paced reliable plus six
datagram controls pass. Longer median changes against the retained-clock image
are private -0.37%, room +0.99%, mixed +0.61%; no statistical speed claim is
made. Both endpoint profile captures pass and all four reports have zero lost
samples. Actual Chrome passes all nine checks, including 5 MiB and datagrams;
connection, staging and output gauges return to zero. The browser bundle rebuild
succeeds with piped output and matches the tested bytes; the initial redirected
esbuild-wasm failure remains a separate recorded build attempt.

The opaque UDP proxy's four checks pass with 20 ms per direction and independent
2% loss. Candidate datagram p99 freshest-state age is 103.63 ms with 17/360
updates missing, versus 5002.47 ms and no missing updates for streams. Bulk
arrives at 9019.36 versus 5076.63 ms. Baseline datagram p99 is 110.42 ms with
12 missing updates. These single runs preserve the freshness/bulk tradeoff;
the fixture is not netem or a native TCP equivalence experiment.

Selected frozen release image:
`1c8e2d562b7e8e1eac07ab633878fd6d95800aa010e05ebdbdd77e7780dca3c0`.
Complete source, archive pin, receipts, failures, scripts and limits:
[session IO screen](reports/webtransport-session-io-screen-2026-11-06.json).

This is a retained reduction of avoidable session state. Full throughput and
memory parity remains unproven, including the large remaining user-space
memory gap and the receiver-pressure mechanism. Client-cooperation and cipher
preference experiments are prepared separately and remain unexecuted.

## Current-image native parity refresh — 2026-11-06

All 18 new native/WebTransport comparisons pass using the same selected `1c8e`
release image in both modes and the unchanged inspection client. Three
12-second repetitions per case rotate transport order. Verified received
private-plus-fanout medians are:

| Workload | Native WebSocket / s | WebTransport / s | WT change |
| --- | ---: | ---: | ---: |
| Private | 1,279,257 | 1,507,285 | +17.83% |
| Room | 1,712,509 | 1,665,885 | -2.72% |
| Mixed | 1,607,213 | 1,623,218 | +1.00% |

Native room rates range 1.603-1.735 million/s, versus WebTransport
1.664-1.678 million/s. Native mixed rates also vary widely, 1.473-1.678
million/s, versus WebTransport 1.621-1.632 million/s. These small samples do
not establish a broad statistical ordering; room throughput parity remains
unproven and its observed median is behind native.

A fresh native 100k control uses the same selected image and fixed capacity
client as the current WebTransport capture. Both verify 400,000 deliveries
and all cleanup gauges. Native ready RSS is 118.875 MiB versus 1889.266 MiB
for WebTransport (15.8929x). Measured server CPU is 1.96 versus 5.96 s
(3.0408x), and client-reported setup is 0.6525 versus 10.7656 s (16.4991x).
This paced capacity phase does not measure maximum throughput. Kernel bytes
remain unavailable, not zero. The older native live-allocation capture is
explicitly historical; no current matched allocation ratio is claimed.

Exact repeat values, workload/CPU identities, counters, latency histogram
bounds, setup/retirement phases, current QUIC allocation scope and receipt
hashes: [current parity report](reports/webtransport-session-io-current-parity-2026-11-06.json).
The full goal remains active and unmet.

## Isolated client read-cooperation diagnostic — 2026-11-06

Nine fixed-order rotated cases compare original frozen client A, unmodified
rebuilt client B, and the same B snapshot with one receiver `consume_budget`
await (C). B/C build from explicit retained source `f9c61c8` in an isolated
source/target tree; the original acceptance clients and selected production
image remain frozen. Every case uses the same read-only first-refusal server
image `737b50de`, the original 32-pair/1024-reader stream workload, buffers,
CPU placement and UDP/thread observer. This is a client-side causal diagnostic,
not server performance acceptance.

A passes two cases, B one and C two. C3 still refuses output with peer stream
credit zero, 8,852,000 B of local send-window headroom and a registered writer.
Its first positive client-drop observation completes 318.281 ms before logged
refusal. All four refusals have peer stream credit zero. Several passing cases
also drop UDP packets; B2 has congestion-window headroom greater than the MTU,
so neither drops alone nor instantaneous cwnd exhaustion explains every close.

Passing C1/C2 generate 1,023,810 and 1,023,811 private updates (99.9814% of plan)
and all 4,000 fanout updates. Their private generation-lag p99 upper bound is
1.048576 ms. These passes do not conceal a large offer reduction, but failed
measured gates remain unavailable and three repetitions establish no failure-rate
ranking. The one cooperation call is not sufficient to fix the failure.

Failed pre-refusal CPU windows average 5.387-5.508 cores; scheduler statistics
remain disabled. Requested sampling cadence is not guaranteed: actual UDP gaps
reach 6.84-9.63 ms and CPU gaps 45.18-47.18 ms. Shorter bursts and serialization
of a migrating endpoint future remain unresolved. All sampled server UDP drops
are zero and every owned-socket set is stable.

Build archives, source/feature/toolchain bindings, exact one-line diff, all nine
outcomes, generated counts, first-refusal snapshots and sampling limits:
[client cooperation report](reports/webtransport-client-read-cooperation-diagnostic-2026-11-06.json).
Next diagnostics distinguish ingress isolation and send-buffer ownership/recovery
without raising resource budgets or replacing fixed-client server acceptance.

## Immediate reliable fanout screen rejected — 2026-11-06

This candidate gives reliable room fanout the same single cancel-safe admission
poll used by private output when the recipient queue is empty and its writer is
not busy. Gated observer sends remain queued. A real 12 MiB fanout under 64 KiB
peer stream credit verifies that partial admission resumes before later private
output. All 254 relay tests and Clippy pass, including existing observer/barrier,
blocked-output timeout and queue ownership checks.

All 18 ordinary 12-second comparisons pass correctness, but median throughput
changes against compact SessionIO are private -0.43%, room -10.17%, mixed -6.60%.
Room server CPU falls from 36.32 to 32.11 s and mixed from 39.71 to 34.51 s;
lower CPU does not compensate for lower verified-delivery throughput. Reliable
paced controls pass 2/3 for each variant, retaining both repeat-3 failures.
Datagram controls pass 3/3 for each variant.

The candidate is rejected; selected source/tests are restored and rebuilt to
`1c8e2d562b7e8e1eac07ab633878fd6d95800aa010e05ebdbdd77e7780dca3c0`.
No memory, browser, freshness or ordinary profile benefit is claimed for it.
Source/test changes, hashes, controls and all failures remain in the
[fanout screen report](reports/webtransport-fanout-immediate-screen-2026-11-06.json).

Moving admission into the publisher changes both synchronous work and lock/batch
behavior. The regression is not attributed to mutex waiting without measurement.
A separate sampled write-lock diagnostic is prepared to distinguish acquisition
wait from held time. Nonblocking admission remains a hypothesis, not a fix.

## Client dedicated-ingress diagnostic — 2026-11-06

Six rotated four-second offers compare unmodified rebuilt client B with client
D, using the same frozen first-refusal server `737b50de`. B has eight shared
runtime workers on CPUs 8–15; D has seven application workers on CPUs 9–15 and
one dedicated endpoint runtime on CPU 8. Both have eight runtime workers within
the same eight-core allowance, plus a coordinator thread allowed on CPUs 8–15.
The server remains four workers on CPUs 0–3. Each process owns one UDP socket;
both arms request the same 4 MiB receive buffer on client and server. Traffic,
certificate pin, payloads, pacing and verification are unchanged.

All six cases pass. B controls also pass 3/3, so D does not demonstrate a
reliability fix. Counts below describe generated source updates; every generated
delivery is verified, with zero duplicate/order errors or unreceived updates.

| Case | Client UDP drops | Sampled client RX peak, bytes | Client / server CPU, s | Private / fanout source updates |
| --- | ---: | ---: | ---: | ---: |
| B1 | 1,788 | 6,681,589 | 17.46 / 13.42 | 1,023,928 / 4,000 |
| D1 | 0 | 934,914 | 16.12 / 13.82 | 1,023,823 / 4,000 |
| D2 | 0 | 177,409 | 16.35 / 13.69 | 1,023,776 / 4,000 |
| B2 | 13,152 | 6,880,576 | 21.03 / 13.54 | 1,023,941 / 4,000 |
| B3 | 9,025 | 7,108,574 | 20.82 / 13.53 | 1,023,783 / 3,999 |
| D3 | 0 | 183,790 | 16.77 / 13.69 | 1,023,840 / 4,000 |

Private generation is 99.978125–99.994238% of the 1,024,000-update plan; B3 also
omits one of 4,000 planned fanout updates. These small cutoff differences are
reported without excluding cases. Server UDP drops are zero throughout. All
six verify 1,089 normal closes and 1,024 source departures, exit successfully,
and return relay connection/session/queue/read-buffer accounting to zero.

D reduces observed client ingress pressure: median sampled RX peak falls
97.33% and measured client CPU falls 21.47%. Median server CPU rises 1.18%.
Private arrival-age p99 bucket upper bounds are B 8.389/16.777/16.777 ms and
D 8.389/8.389/4.194 ms; fanout bounds are B 8.389/16.777/16.777 ms and
D 8.389/8.389/8.389 ms. These are instrumented diagnostic results, not ordinary
server performance measurements. D jointly changes application-pool allocation
and ingress isolation; a seven-worker shared-runtime control was not run.
The result does not identify a particular lock or prove endpoint scheduling is
the unique cause of the earlier intermittent refusal.

Canonical B/C/D source archives match their manifests; B→D changes exactly two
files and does not include C's added receiver cooperation. The shared build
source is restored to C, original A/B/C artifacts remain unchanged, and selected
production image `1c8e2d56` remains unchanged. Exact generation fractions, lag,
last submissions, per-role CPU, placement, sampling gaps, source/build hashes,
receipts and cleanup limits are in the
[dedicated-ingress report](reports/webtransport-client-dedicated-ingress-diagnostic-2026-11-06.json).
Longer offers are recorded separately; this screen establishes no
server-performance or reliability fix.

## Live-kernel capacity accounting — 2026-11-06

Two fresh ordinary 100,000-session runs use the same selected `1c8e2d56`
server and fixed capacity client. Both verify 400,000 measured deliveries and
return all connection, session and resource gauges to zero. Measured RSS is
116.418 MiB for native WebSocket and 1890.367 MiB for WebTransport. Server CPU
is 2.03 versus 6.27 s; setup is 0.7033 versus 10.9594 s. RTT p99 histogram
upper bounds are 0.524288 versus 2.097152 ms. This paced capacity phase is not a
maximum-throughput test, and one run per transport establishes no confidence
interval.

The kernel's readable BTF type information supplies a separate, reproducible
protocol-object payload lower bound. An archived BTF image and matching boot,
kernel and version identities before and after both runs bind the sizes to the
live socket counts. Native owns 100,000 unique established IPv4 TCP sockets;
`tcp_sock` is 2432 B, giving a 243,200,000 B (231.934 MiB) C-payload lower bound.
WebTransport owns one UDP socket; the base `udp_sock` is 1344 B. The UDP census
combines families, so the base type is a conservative bound rather than a claim
of the exact IPv6 allocation size.

These are C structure payloads, not physical slab sizes or complete kernel
memory. Socket buffers, allocation rounding, socket/file/inode counts, FD
tables, routes and other shared state remain outside this bound. Neither a
complete kernel total nor a userspace-plus-kernel total or ratio is claimed.
Logical cleanup does not require allocator RSS to return immediately.

The earlier [conditional type-size report](reports/webtransport-live-kernel-object-payload-2026-11-06.json)
remains unchanged because its older capacity receipts lack boot IDs. Fresh
boot-bound receipts, full phase metrics, socket censuses, archived BTF, parser
selfchecks, source/binary hashes and limits are in the
[live-kernel capacity report](reports/webtransport-live-kernel-capacity-2026-11-06.json).
Memory parity remains unmet.

## Longer client ingress diagnostic — 2026-11-06

Six new rotated B/D cases extend only the offer duration from four to sixteen
seconds. The frozen clients, diagnostic server, rates, 1024 readers, CPU
allowance, requested buffers and one-second drain remain unchanged. The new
plan is 4,096,000 private and 16,000 fanout source updates, or 20,480,000
deliveries. All six pass, verify every generated delivery and clean up.

| Case | Client UDP drops | Sampled client RX peak, bytes | Client / server CPU, s | Generated deliveries |
| --- | ---: | ---: | ---: | ---: |
| B1 | 12,814 | 6,953,743 | 80.28 / 54.39 | 20,479,866 |
| D1 | 0 | 536,038 | 67.14 / 54.89 | 20,479,937 |
| D2 | 0 | 860,809 | 66.82 / 55.25 | 20,479,998 |
| B2 | 12,794 | 7,610,586 | 82.39 / 54.56 | 20,479,961 |
| B3 | 12,341 | 6,726,780 | 82.67 / 54.84 | 20,479,837 |
| D3 | 0 | 2,872,072 | 65.99 / 55.19 | 20,479,980 |

All cases generate all 16,000 fanout updates. Private generation is
99.9960205–99.9999512% of plan; cutoff differences remain visible and no cases
are excluded. Dedicated ingress retains its observed pressure reduction over
the longer offer. Its thread averages 0.810–0.819 core over roughly 17-second
measured windows, including drain. That average does not rule out shorter
single-core saturation, and D3's 2.87 MB sampled queue peak remains material.

Controls also pass 3/3. This is neither proof of a reliability fix nor a server
performance improvement, and allocation/isolation remain a joint treatment.
It does not replace fixed-client ordinary server comparisons. All six raw
receipts, immutable four-second references, duration-only recipe checks,
generation/lag/age histograms, per-role CPU and observer limits are in the
[long ingress report](reports/webtransport-client-dedicated-ingress-long-diagnostic-2026-11-06.json).

## Remaining memory ownership — 2026-11-06

A source/receipt review of the selected SessionIO ready heap verifies
1,848,987,316 requested live bytes. This reuses the existing selected-image
allocation capture; it is not a new heap measurement. The largest owners are:

| Owner | Live objects | Requested MiB |
| --- | ---: | ---: |
| Quinn connection allocation | 100,000 | 509.64 |
| Packet-encryption keys | 400,068 | 219.76 |
| Header-protection keys | 200,000 | 48.83 |
| Relay application tasks | 100,000 | 195.31 |
| Rustls sessions and final state boxes | 200,000 | 119.78 |
| HTTP CONNECT capsule tasks | 100,000 | 48.83 |

The 5344-byte Quinn allocation includes protocol and high-level state,
synchronization and handles; its size is not proof that all those bytes are
avoidable. Likewise, aggregate vector and lookup-table sites require deeper
attribution. Requested-live allocation bytes and ordinary RSS are distinct
measurements and are not added together.

The current key classes alone request 268.59 MiB, larger than the entire
116.42 MiB native process RSS in the fresh same-boot capacity capture. Small
adapter or pool fixes therefore cannot reach this RSS target while the current
protocol, crypto and runtime representations remain. This describes the
selected implementation footprint, not a universal QUIC lower bound. Direct
`quinn-proto` integration still needs to earn its benefit by changing measured
state or scheduling; deleting wrappers alone proves no saving.

The next memory hypothesis is the staged ChaCha preference CPU/throughput
screen, followed by compact Ring key representation only if whole-workload
cost is acceptable. Cipher preference alone does not shrink current Ring
objects. No new crypto, key deferral or discarded TLS/capsule behavior is
proposed. Exact allocation sites, counts, source hashes, current phase-correct
RSS and limits are in the
[memory ownership review](reports/webtransport-session-io-memory-ownership-2026-11-06.json).

## Stronger refusal probe did not reproduce — 2026-11-06

A separate current-source diagnostic image adds first-refusal-only ownership,
packetization, selective-ACK, retransmission, timer and event-queue snapshots.
Two proto partition/budget tests, two output queue/partial-record unit tests and
the existing blocked-output and partial-write integration tests pass. The
diagnostic image is frozen separately; all seven temporary source files and
the selected `1c8e2d56` image are restored and hash-verified before traffic.

All six prescribed original-client attempts pass without a logged refusal.
Client sampled UDP drops range 290–10,415 and RX peaks 6.394–7.084 MB, while all
sampled server drops are zero. Every case generates all 4,000 fanout updates
and 99.9842–99.9900% of planned private updates, verifies every generated
delivery, closes 1,089 sessions, checks 1,024 departures and returns resource
gauges to zero. The fixed six-attempt series ends without extension.

The new send-buffer/recovery snapshot and pre-refusal correlation are therefore
unavailable. This negative reproduction does not explain or fix the earlier
intermittent closes. Drops alone remain insufficient evidence, and the hook
does not cover earlier reservation rejection or an already-closed queue.
Actual sample gaps, disabled scheduler-statistics limits, all receipts and
44 verified immutable build/source/log references are retained in the
[negative probe report](reports/webtransport-stronger-send-state-negative-probe-2026-11-06.json).

## ChaCha preference screen rejected — 2026-11-06

The server-only TLS policy candidate prefers ChaCha20-Poly1305 while retaining
AES suites, the certificate resolver, ALPN and transport configuration. Two
separate debug diagnostics verify 64 actual negotiations each: selected
baseline AES-256-GCM and candidate ChaCha20-Poly1305. Debug timings are excluded.
The first preflight is preserved as a controller failure: the shared transport
helper reset the requested log filter. A new diagnostic-only wrapper applies
the filter after that helper; ordinary preparation remains unchanged.

All 18 ordinary 12-second comparisons pass, using unchanged inspection clients
and three rotated-order repetitions for each workload. Verified received
private-plus-fanout medians are:

| Workload | Selected AES / s | ChaCha candidate / s | Change |
| --- | ---: | ---: | ---: |
| Private | 1,467,264 | 1,123,080 | -23.46% |
| Room | 1,564,243 | 1,379,075 | -11.84% |
| Mixed | 1,514,543 | 1,279,835 | -15.50% |

Median server CPU rises from 40.00 to 41.42 s, 36.41 to 38.95 s and 40.27 to
42.25 s respectively, despite fewer verified deliveries. Server CPU per
delivery rises 35.28%, 21.48% and 24.16%; client CPU per delivery also rises.
The cipher changes both endpoints' work, so the throughput cost is not
attributed exclusively to server computation. Reliable paced controls pass
3/3 per image, and datagram controls pass 3/3 per image.

Reject this cipher preference and do not proceed with the proposed ChaCha Ring
compaction. Cipher choice alone changes no Ring key representation, and no
memory saving is claimed. Source and selected `1c8e2d56` binary remain restored.
No candidate browser, loss/bulk freshness or memory acceptance was needed for
this rejected CPU screen. All 32 completed cases, the failed logging preflight,
source/build hashes, exact distributions, both-endpoint CPU, latency and
datagram observation limits are in the
[cipher screen report](reports/webtransport-chacha-preference-screen-2026-11-06.json).

## Current connection and task layouts — 2026-11-06

A source-identical release compiler diagnostic leaves the selected image and all
373 build inputs unchanged. Checked field sums and repeated layouts connect the
5344-byte heap allocation to a 5328-byte Quinn owner plus its 16-byte Arc header.
Of that allocation, 4656 B is protocol state and 688 B is the wrapper, mutex,
shared notifications, header and padding. Removing the wrapper alone cannot
remove the protocol owner or prove a net saving after replacement storage.

The current application task is 2048 B: 32 B header, 1888 B core, 48 B trailer,
80 B padding. Its outer future is 1864 B and session future 1792 B. Established
state peaks at 1791 B versus 1207 B for join setup; the active writer is 544 B
and select tuple 880 B. Cold handshake boxing is already implemented. Further
setup boxing therefore has no measured large saving to claim. The separate
capsule task is 512 B and cannot be counted as removed by relocating its parser.

Exact field/variant data, sum/repetition checks, toolchain, diagnostic flags and
source bindings are in the
[current layout report](reports/webtransport-current-state-layout-2026-11-06.json).
These are storage-layout findings, not a new performance improvement. An owned
session-state rewrite and a smaller routing continuation remain hypotheses.

## Guarded inline assembler rejected — 2026-11-06

This distinct candidate retains the earlier one-chunk inline storage but restores
the original guarded partial-read algorithm through a safe inline/heap peek
guard. All 323 protocol tests, 253 relay tests (one existing ignored), Clippy and
matched 100k routing/cleanup checks pass. Ready live allocations fall
1,847,927,909→1,793,103,409 B (52.2847 MiB); ordinary ready RSS falls
1889.796875→1832.625 MiB (57.171875 MiB).

The 300,000 legacy 224-byte assembler roots disappear, leaving 65 two-slot spill
allocations totaling 7280 B. Connection and receive objects grow by 3.2 MB and
9.6 MB respectively. Their direct net saving is 54,392,720 B; another 431,780 B
of total difference is not attributed to the representation. Neither arm retains
captured assembler staging at done.

All 18 ordinary comparisons pass, but median verified throughput changes private
+0.3174%, room -0.2085%, mixed -1.7567%. Restoring the guarded read path does not
remove the repeated mixed-workload cost. Reliable paced controls pass 0/3 for the
baseline and 2/3 for the candidate; all four failures remain visible and establish
no failure-rate ranking. Datagram controls pass 3/3 each. Both endpoint profiles
pass with zero lost samples in four reports; no instruction/cache cause is claimed.

Reject the candidate and retain the selected `1c8e2d56` source/image. Browser and
loss/bulk freshness gates are skipped for this rejected variant. Two controller
failures (a post-success return-value assumption and unavailable SOCK_DIAG) are
recorded separately from application results; corrected allocation-only captures
do not measure kernel memory. Full scripts, hashes, profiles and limits are in
the [guarded assembler report](reports/webtransport-inline-peek-screen-2026-11-06.json).

## Compact protocol unit notifications retained — 2026-11-06

The protocol's internal application-event queue only receives four payload-free
notifications, but previously stored the 56-byte public event enum. Store a
private one-byte enum and convert back when polling. All five producer sites,
duplicates, FIFO order and queued/stream/error priority remain intact. Public
events, endpoint events, flow control, resource limits and the wire format are
unchanged. This reduces stored metadata without adding a queue, task or cache.

At 100,000 sessions, event buffers change from 100,000 x 224 B to 100,000 x 8 B:
21,600,000 B (20.5994 MiB) directly saved. The core connection allocation stays
5344 B. Matched ready live allocations fall 1,848,346,869→1,825,903,840 B,
a total 21.4033 MiB saving; the additional 843,029 B is not attributed to the
event change. Ordinary ready RSS falls 1889.9961→1871.2305 MiB in that pair.

All 18 incremental reliable comparisons pass. Private, room and mixed medians
change -0.2601%, +1.1916% and -0.0671% versus `1c8e2d56`; ranges overlap, so
no general speedup is claimed. Datagram controls pass 3/3 each. Original-client
reliable controls pass 3/3 baseline and 2/3 candidate: the candidate's first
EOF/slow-consumer failure remains failed with its measured rate unavailable.
The preceding identical-baseline 0/3 control result shows a pre-existing
instability but does not classify or erase this failure. Six additional controls
with the same frozen dedicated-ingress client pass, three per server image,
without unreceived generated updates or measured UDP-drop deltas. Those runs
provide additional functional coverage and do not replace original-client
acceptance or establish a failure-rate improvement.

All 322 protocol tests and 253 relay tests pass (one existing relay test ignored),
as does all-target Clippy. Archive verification covers 300 files, six pinned
archives and five complete patches. Both endpoint profiles pass with zero lost
samples in four reports. Actual headed Chrome 157 passes all nine browser checks,
including typed CBOR, datagrams, 5 MiB delivery and three clean closures; console
errors/warnings and final gauges are zero, and task-owned browser/fixture
processes are gone.

Four user-space UDP-proxy freshness cases pass at configured 20 ms each direction
and 2% loss. Candidate datagram p99 freshest-state age is 110.47 ms with 19/360
missing updates, versus 5020.89 ms and none missing for reliable streams. Bulk
arrives at 9660.22 versus 5037.37 ms. This preserves the freshness/bulk tradeoff;
single-pair proxy results are not TCP/netem equivalence or a tail SLA.

A fresh native comparison uses the same final `f1811d2c` image and frozen client
in both modes. All 18 cases pass; three 12-second repetitions rotate transport
order. Verified received medians are:

| Workload | Native WebSocket / s | WebTransport / s | WT change |
| --- | ---: | ---: | ---: |
| Private | 1,284,693 | 1,507,742 | +17.36% |
| Room | 1,685,258 | 1,668,916 | -0.97% |
| Mixed | 1,615,396 | 1,619,519 | +0.26% |

Native room rates range 1.512–1.697 million/s and mixed 1.582–1.797 million/s.
These small, variable samples do not prove room-throughput parity or a broad
ordering. Both modes also pass a fresh same-boot 100k capacity run, each checking
400,000 paced deliveries. Ready RSS is 115.3242 versus 1872.1367 MiB (16.2337x
process RSS); server CPU is 1.96 versus 6.07 s, setup 0.6671 versus 10.5145 s,
and retirement 0.6297 versus 1.9924 s. RTT p99 bucket upper bounds are 0.524288
versus 1.048576 ms. Capacity traffic is not maximum throughput. Native WebSocket
is plaintext; QUIC includes TLS.

Same-boot BTF and socket counts separately bound protocol C-object payloads at
243.2 MB for 100,000 TCP sockets and 1344 B for one UDP socket. These exclude
socket buffers, slab rounding and other kernel storage; no complete kernel or
userspace-plus-kernel total is claimed.

The ordinary root build is byte-identical to the frozen tested image
`f1811d2cd2dc6736b48fe0a1fdf8459209e0530a1a48ae930f56ff2cec0a7851`.
Retain this incremental memory improvement. Full throughput/memory parity and
original-client reliable-load acceptance remain unresolved; the pivot is not
accepted. Exact distributions, CPU/latency/age metrics, all failures, build and
source identities, reproduction scripts and scope limits are in the
[unit-notification report](reports/webtransport-proto-unit-events-screen-2026-11-06.json).

## Eager routing continuation rejected — 2026-11-06

Move the immediate receive/publish/drop prefix outside its suspended routing
continuation, preserving both immediate-await callers and accepted fanout
retirement on cancellation. The release compiler reduces the main session task
2048→1920 B; the writer remains 544 B. At 100,000 sessions the main task owners
save exactly 12,800,000 B, while total ready requested-live heap falls
1,826,122,955→1,813,170,708 B (12.35 MiB). Other allocations account for the
remaining 152,247 B. Output, established IO and capsule allocations are unchanged.
The separate ordinary capacity pair lowers ready RSS by 6.36 MiB. All four
capacity/heap runs pass routing, closure and full resource retirement checks.

All 18 ordinary comparisons pass, but every same-repeat candidate rate is lower.
Private, room and mixed medians change -0.6083%, -0.0814% and -1.5517%; datagram
controls pass 3/3 each with received-offer median -0.5936%. Original-A reliable
controls pass 1/3 on both arms. Four EOF failures remain failed with measurements
and medians unavailable; this establishes no failure-rate or causal ranking.
Ordinary done gates assert their documented gauges, but observe 64–177 remaining
QUIC connections at that point. Full retirement evidence comes from the separate
capacity runs, not that earlier inspection gate.

The actual cancellation test, 254 relay tests (one existing ignored), all-target
Clippy and release build pass. Reject the modest memory saving with the lower
mixed-throughput median; retain selected `f1811d2c` source/image. No browser,
freshness or CPU-profile acceptance run is needed for this rejected candidate.
No compiler-layout/cache cause is inferred. Exact distributions, both-endpoint
CPU and latency metrics, failed receipts, source/binary bindings and reproduction
scripts are in the [routing screen report](reports/webtransport-route-input-helper-screen-2026-11-06.json).

## Endpoint readiness attribution captured — 2026-11-06

Rebuild canonical client B with a passive endpoint recorder, preserving the
original eight shared workers, CPU placement, four-second traffic offer, buffers,
queues and verification. Keep the frozen first-refusal server `737b50de` fixed.
Every endpoint poll and known self-wake is recorded; one in 64 polls additionally
times endpoint-lock acquisition, receive/dispatch work and endpoint events, with
bounded nested batch/send samples. Unsampled counters are unavailable. Clock
anchors bound the join to the server warning and owned-socket UDP observations.

The first bounded series passes three times, then captures a refusal with a
complete 8192-row dump. It retains only 90 ms before the warning, while UDP drops
started about 2.4 seconds earlier. A separate capacity-only recorder change to
262,144 rows closes that history gap. This second fixed-budget series passes
five times, then captures another refusal. All six files retain their complete
histories without overwrite. Both failed clients exit naturally with code 1 and
close code 1013; neither is killed by the controller. Counts describe stopped
diagnostic series, not failure-rate estimates or original-A acceptance.

The full failed history has 162,366 polls. First client UDP drops map to
2.938080–2.942877 s, with the warning at 4.003825–4.003826 s. In the 250 ms before
first drop, 4180 known-ready entries have a maximum wake-to-poll upper bound of
133.486 us; 88 detailed polls have an endpoint-lock acquisition maximum of
130 ns. In the 250 ms before warning, all 3897 entries consume known self-wakes,
with a maximum 159.486 us; 61 detailed lock samples reach at most 90 ns.
That quarter-second partitions into 193.625 ms entry-to-wake-before, 0.240 ms
wake-call brackets and 56.135 ms wake-after-to-next-entry. These are elapsed
intervals, not CPU duty; shared wake-call brackets cannot be counted twice.

The client UDP queue builds and drops while the endpoint repeatedly progresses.
The warning again has zero stream credit, with connection and send-window credit
available. Passing attempts also experience UDP drops during their conservative
source-derived offer cores. This evidence does not support a long delay after a
known self-wake or a sampled endpoint-lock spike as the immediate onset/refusal
explanation. Untimed external readiness, unsampled locks, connection processing,
ordered reassembly, application reads and credit transmission remain unresolved;
no unique cause is established.

The recorder, larger working set and UDP/CPU observer perturb these diagnostics.
The 64-poll detail cadence can also alias with the receive limiter's 256 nonempty
cycle adaptation. No sample extrapolation, ordinary timing ranking, server win
or reliability fix is claimed. Selected `f1811d2c` source/image remains retained.
The [attribution report](reports/webtransport-endpoint-ready-attribution-2026-11-06.json)
is a byte-identical copy of the independently reviewed full-history findings,
with all six outcomes, sample domains, source/binary bindings, generation limits,
UDP identities, timeline bounds and reproduction-reader references.

## Untraced client receive-target sensitivity pilot — 2026-11-06

A separately rebuilt, untraced canonical B client changes only Quinn's receive
fairness target from 50 to 250 us. All 332 source files, relocated dependency
metadata and toolchain are checked; only that constant differs. This is an
explicit fivefold fairness-target increase, not a hard time cap: the limiter
checks after whole native-message batches and estimates work between sampled
nonempty cycles. Queues, byte/flow-control limits, buffers, eight shared workers,
CPU placement, verification and the frozen `737b50de` server stay fixed.

Four diagnostic cases run in predetermined 50/250/250/50 order without an endpoint
recorder. The first 50-us case passes with zero UDP drops. Both 250-us cases pass
with zero drops. The last 50-us case naturally exits 1 with close code 1013 and
the familiar zero-stream-credit refusal; its measured counts, CPU and histograms
remain unavailable. All four outcomes are retained without extending the pilot.
This split is not a failure-rate estimate or a demonstrated reliability fix.

All generated deliveries in the three passes are verified without duplicates,
reordering or unreceived messages. Private source shortfalls are 132/69/209 of
1,024,000 planned updates; fanout generates 4000/4000/3999 of 4000. Observed client
CPU is 17.18/20.49/20.64 s and server CPU 13.18/13.36/13.13 s in pass order.
Private generation-lag p99 upper bounds remain 1.048576 ms; fanout bounds are
0.524288/1.048576/2.097152 ms. Fanout arrival-age p99 upper bounds stay 8.388608 ms,
while recorded maxima are 11.664/17.052/22.166 ms. These are diagnostic values,
not an ordinary CPU-efficiency ranking or a repeated-baseline median.

Do not adopt the larger default from this pilot. It supplies no clean integrated
improvement or server-performance proof, and the fairness/lag tradeoff remains.
Selected `f1811d2c` and protected root/C/original-client sources and artifacts
remain unchanged. Connection-event consumption, ordered reads, application
consumption and credit transmission remain the next causal boundaries to trace.
The [receive-target pilot report](reports/webtransport-client-receive-target-pilot-2026-11-06.json)
is byte-identical to the independently reviewed analysis and retains source
audits, exact images, all histogram objects, both-endpoint CPU, socket identities,
observer limits, failed null measurements and raw/reproduction references.

## Compact established application state retained incrementally — 2026-11-06

Retain the three frozen compact application-state files at code commit
`f01198156047a93e9233b78d19e44ea9d529bd87`. The normal root release image is
byte-identical to the measured candidate, SHA256
`91337694ecef14ff3bb80bf37d13786aa5d4653e462096aeb3ddd92b71e19ea1`.
The [integration receipt](../artifacts/webtransport/compact-application-v1-integration.json)
binds the live sources, frozen prototype, normal build log, code commit and measured
image. **The full goal remains ACTIVE / NOT ACCEPTED.** This is an incremental
memory improvement with exercised behavior preserved; it does not establish full
native WebSocket parity, a reliability fix, a unique allocator/cache/read-credit
cause, or results on all hardware. CPU costs remain measured separately from the throughput and memory criteria.

The established poll is the measured layout maximum. Explicit application
writer/input state replaces large stored async selection state; the output queue
owns a registered waker, and an inline same-task drain flag replaces the local
finishing token. The complete main task allocation shrinks from 2,048 to 1,792
bytes, the output Arc from 288 to 272 bytes, and the nested session future from
1,792 to 1,560 bytes. Its active variant is 1,559 bytes, versus admission 1,015,
drain 751 and final close 831. Those nested sizes are not additional heap owners.
No single writer-size subtraction or cache explanation is used to claim the gain.

At 100,000 sessions, the matched diagnostic heap captures show requested live bytes
falling from 1,826,016,848 to 1,787,692,258: **38,324,590 bytes, or 36.55 MiB,
less**. Source-bound allocation stacks identify 25,600,000 fewer bytes in main task
cells, 1,600,000 fewer output-Arc bytes, and removal of 100,000 local 112-byte
finishing-token allocations (11,200,000 bytes); other allocations increase by
75,410 bytes in aggregate. The established-I/O Arc and capsule task remain.
All five attributed per-session owner classes disappear after retirement in this
diagnostic pair. This is not an exhaustive owner census or proof of long-term leak
absence, and diagnostic requested-live heap is separate from normal-allocator RSS.

Preserve the original normal-allocator done observation: candidate RSS exceeds
baseline by 189,148 KiB, **184.71 MiB**, with zero session/budget gauges. The absent
attributed per-session heap owners belong to the separate tcmalloc diagnostic pair. The separate two-wave supplement does not
reproduce that gap: candidate done RSS is lower by 26,736 and 26,520 KiB in its
two waves. Both arms retain a large resident footprint; the second population adds
only about 8–9.4 MiB at sampled ready/measured gates, consistent with reuse of
retained process capacity. Gate samples do not establish continuous peak memory,
delayed-idle decay, a unique allocator cause or a leak verdict.

All 18 ordinary fixed-client runs pass with unchanged verification and exact
counter gates. Verified-received median changes versus frozen baseline `f1811d2c`
are +0.0252% private, +0.6945% room only and +1.9828% mixed. These are descriptive
local differences, not a component ablation. Original frozen paced client A
(`b1fea5d6`) passes 1/3 on baseline and 3/3 on candidate; the two baseline EOF
outcomes retain null measurements and there is no comparative median or
failure-rate ranking. Datagram controls pass 3/3 on each arm with received-offer
median -0.3494% and overlapping ranges. The original causal read-credit failure
remains unresolved, and the separate diagnostic 250-us client is not retained.

The fresh same-image comparison uses the retained relay, one frozen `3af40bb6`
inspection client, and three rotated 12-second repetitions per case and transport.
All 18 receipts pass. Rates below count verified healthy plus fanout deliveries,
and the report retains each component, both endpoints' CPU, and all latency
histograms separately.

| Case | Native WebSocket min / median / max deliveries/s | WebTransport min / median / max deliveries/s | Median change |
| --- | ---: | ---: | ---: |
| Private | 1,267,322 / 1,285,029 / 1,326,260 | 1,502,955 / 1,511,186 / 1,524,226 | +17.5994% |
| Room only | 1,556,926 / 1,608,071 / 1,633,123 | 1,690,139 / 1,693,727 / 1,731,428 | +5.3266% |
| Mixed | 1,419,026 / 1,605,249 / 1,607,395 | 1,635,838 / 1,648,776 / 1,664,125 | +2.7115% |

The mixed private/fanout split differs materially between transports, so its total
does not establish component parity. Native WebSocket is plaintext and QUIC uses
TLS; these results include that difference. They do not resolve capacity memory
or establish parity outside the measured local workloads.

The fresh same-image 100,000-session capacity pair verifies 400,000 measured
deliveries per mode and retires all session/QUIC/budget gauges to zero. It uses the
same frozen capacity client `ec893ebc` and one boot. At ready, relay RSS is
119,684 KiB (116.87890625 MiB) for native WebSocket versus 1,886,128 KiB
(1,841.921875 MiB) for WebTransport: **15.759232646×**, or 1,725.04296875 MiB
more. At measured it is 119,684 versus 1,888,696 KiB (1,844.4296875 MiB),
**15.780689148×**, or 1,727.55078125 MiB more. This large process-memory gap keeps
the full goal open.

WebSocket/WebTransport setup is 0.624565644/10.4632178 seconds, measured relay CPU
1.95/6.05 seconds (client 2.42/6.81), and retirement 0.616366588/2.046467136
seconds. Measured round-trip p99 histogram upper bounds are 0.524288/1.048576 ms.
This is paced capacity evidence, not a maximum-throughput test. The same-boot BTF
record gives a protocol-object payload lower bound of 100,000 × 2,432-byte
`tcp_sock` = 243,200,000 bytes versus one 1,344-byte `udp_sock`. It excludes socket
buffers, slab physical rounding, file/inode/FD allocations and other kernel
memory. There is no full kernel total or combined RSS-plus-kernel total.

Separate `cycles:u` diagnostic profiles cover both endpoints on both arms; all four
symbol reports have zero lost samples. They describe relative sampled shares,
not ordinary timing, absolute cycle costs, a component benefit, or a causal
read-credit/cache explanation. Recorded validation totals 258 passing tests and
one existing ignored test, including six required actual reader/queue/waker contract checks (five new and one
strengthened existing test), plus all-target Clippy with warnings denied and the normal release
build.

Actual headed Chrome 157 passes nine top-level checks and three orderly closures
with no console errors or warnings. Session, connection and budget cleanup gauges are zero, with the empty global channel
count remaining one. The screenshot was visually inspected, and all owned browser/server/HTTP processes
are gone. Source-map contents and exact current JS sources bind to the reused
browser bundle; JS did not change and that bundle was not rebuilt.

All four user-space proxy freshness cases pass their transport and fixture checks.
Each direction configures 20 ms delay and independent 2% Bernoulli loss. Candidate
datagram freshest-state p99 is 124.278647 ms with 15 of 360 updates not received
within the observation window and 9,350.533092 ms reliable-bulk arrival; candidate
stream values are 5,041.402184 ms, zero missing and 5,072.125325 ms bulk arrival.
Baseline datagram values are 113.216705 ms, 14 missing and 9,413.422873 ms bulk;
baseline stream values are 4,701.576815 ms, zero missing and 4,739.968426 ms bulk.
These single diagnostic pairs do not rank implementations, establish statistical
tails or native TCP/netem equivalence, or attribute missing updates to network
loss. Seeds 42/43 do not produce identical packet trajectories.

Independent source/evidence review recommends incremental retention with no
concrete correctness blocker. Its review completed before the fresh native screen,
which is recorded separately above. No hosted deployment or production rollout
is claimed. The [compact application report](reports/webtransport-compact-application-screen-2026-11-06.json)
retains the per-run rates, both CPU endpoints, complete small component histograms,
null failed measurements, source/binary/build/integration bindings, reproduction
instructions and hashed raw references. The full 603,310-byte terminal screen
remains referenced for every raw counter map rather than duplicated into the
bounded report.

## Selected-peer credit and sender backlog captured — 2026-11-06

A separately reconstructed canonical-B 50-us client traces queue service,
connection processing, ordered reads, application verification and credit
encoding/transmit association for 32 private recipients and eight fixed fanout
sentinels. A separately named diagnostic server adds a joined relay ID and an
earlier literal refusal timestamp to the frozen `737b50de` lineage. These are
diagnostic images; retained product `91337694` and its configured limits are
unchanged. Numeric relay ID and stream ID provide the cross-process join.

Seven offers complete the predefined experiment: passing initial control, four
marked passes, one marked refusal and passing final control. No further marked
offer follows that first refusal. Two tooling issues are preserved separately:
the first parser omitted valid reason-0 state wakes, and a later controller
recognized EOF but initially missed the equivalent natural end-gate exit wording.
Stored data was reparsed, only the unused budget resumed, and the failed workload
still has no final measurement or reconstructed generated counts.

The failing private recipient, stream 4, retains all 2909 selected records with
no overwrite. At the literal warning the server's stream maximum and admitted
write frontier are both 25,896,519 B. The client's latest encoded credit maximum
is also 25,896,519 B, with local socket acceptance recorded. This demonstrates
agreement on that value, without assigning receipt or ACK to a particular
duplicate encoding. Bracketing consumed-byte observations place its unadvertised
read advance at 103,638–133,462 B, below the existing 156,250 B renewal threshold.
The observations do not support a lost newly queued credit update.

The selected receiver continues ordered transport and verified application
progress. Earlier ordered-gap branch visits follow shared client UDP pressure,
but their cumulative count is not a current missing-packet count: the last
pre-warning snapshot has consumed prefix equal to highest received end and no
buffered bytes. The shared socket has 12,034 drops before refusal, without
connection-specific loss attribution. Sampled service/readiness intervals are
short; the final eight milliseconds contain no sampled queue/wake/lock pairs,
so that interval remains unavailable rather than zero-delay evidence.

There is a rigorous sender-backlog lower bound. The admitted stream frontier
25,896,519 B exceeds lifetime gross returned UDP output 25,569,832 B by
326,687 B. Every returned transmit counts its full bytes before `try_send`,
including headers, control, retransmissions, GSO and other streams. The output
submission gate prevents this stream's admission frontier advancing across the
two warning snapshots. Consequently at least 326,687 admitted stream bytes have
not appeared in accounted returned output or been locally accepted. This is not
the exact unsent-byte value and does not classify the full 1,130,061 B retained
send storage. The packetizer's exact unsent/retransmission/ACK-retention partition
and its pacing, congestion or service cause remain unresolved.

This narrows the failure to exhausted write admission and a bounded output queue
behind a nonzero sender backlog, with receiver credit progress continuing. It
does not identify a unique upstream scheduling or recovery defect. The
[peer-credit attribution report](reports/webtransport-peer-credit-attribution-2026-11-06.json)
is byte-identical to the reproduced analysis and preserves all seven offers,
literal-clock bounds, counter-source proof, sample domains, tooling history and
raw/source hashes. Independent read-only analysis corroborates the critical
credit/progress observations. Full memory parity remains open.

## Indexed concrete driver scheduler rejected — 2026-11-07

The resumed custom-driver experiment first replaces per-connection Tokio driver
spawns with explicit indexed driver ownership: one owner per application-runtime
worker, reusable connection slots, deduplicated readiness tokens, bounded ready
batches, and stale-token protection. This uses concrete driver objects rather
than the previously rejected FuturesUnordered grouping. It retains Quinn's
protocol state, shared stream-state mutexes, endpoint event queues, timers and
UDP implementation. It is a scheduling treatment, not the completed direct
protocol-state/I/O ownership experiment.

Both ordinary release images were built in the same isolated source and target
directories from HEAD `48e02ba8`; six source files differ. The product checkout
and selected `91337694` image remain unchanged. Three rotated 12-second repeats
per arm and case all pass content, routing, FIFO and counter checks:

| Workload | Baseline median deliveries/s | Indexed median deliveries/s | Change |
| --- | ---: | ---: | ---: |
| Private | 1,486,312 | 1,337,836 | -9.99% |
| Room | 1,701,989 | 1,621,494 | -4.73% |
| Mixed | 1,632,433 | 1,635,340 | +0.18% |

All private and room candidate samples are below their corresponding control
ranges. Mixed ranges overlap, and aggregate totals do not prove component parity.
Both 100,000-session capacity runs verify 400,000 measured deliveries, 100,000
orderly closes, 750,000 departure checks, and all exact final cleanup gauges.
Ready RSS is 1,842.0977 versus 1,823.9609 MiB: 18.1367 MiB saved, about 0.98%.
This single pair is not a confidence interval or an allocation-owner attribution.
Setup is 11.0925/10.7250 seconds, retirement 2.0861/2.1069 seconds, and paced
measurement server CPU 6.04/5.92 CPU-seconds. RTT p99 histogram upper bounds are
1.048576/2.097152 ms. Capacity pacing is not maximum throughput.

Three scheduler contract tests pass. The serial full relay suite passes 258 tests
with one existing ignored, including all 12 actual UDP/TLS/HTTP3 tests exercising
the candidate server driver. All-target Clippy passes with warnings denied. The
first concurrent full suite stopped on a native WebSocket origin admission-budget
assertion; that receipt is preserved, and the serial rerun passes. No causal fix
for that assertion is claimed.

Both endpoint profiles on each arm have zero lost user-cycle samples. They show
active VAES/AVX2 crypto routines in both versions, but do not identify a unique
cause for the scheduler regression. Shared cooperative budgeting within each
owner turn is part of this treatment; it has not been independently ablated.

Reject this scheduler treatment. Additional paced/datagram/browser/freshness
acceptance and a candidate heap capture were not run after its throughput
selection gate failed. A complete direct quinn-proto owner with changed stream
and I/O ownership remains unfinished; this result neither proves nor disproves
its potential. The goal remains active. The [indexed driver report](reports/webtransport-indexed-driver-screen-2026-11-07.json)
binds both binaries, patch, manifests, fixed clients, raw measurements, all phase
checks, profiles and original test failure. No product code or deployment changes.

## Direct protocol owner implemented; relay integration pending — 2026-11-07

The next experiment now has a working, isolated local-owner QUIC/HTTP3 path.
Unlike the rejected indexed scheduler, this code owns `quinn-proto` connection
state directly. Application stream polls and packet processing use local-only
`Rc<RefCell>` handles, with no connection mutex and no asynchronous write/read RPC.
A shared deadline index and one endpoint timer replace per-connection Sleep
objects. Pending UDP ciphertext retains exact ownership until socket acceptance.
The existing QUIC/TLS core and SETTINGS, CONNECT, stream-header and capsule
parsers remain unchanged. This is a singleton-owner correctness prototype, not
an integrated or timed multiworker relay.

Eight actual network contracts pass: encrypted 1 MiB stream transfer and
bidirectional datagrams with full retirement; pending-read cancellation;
flow-blocked write cancellation preserving the original suffix; endpoint shutdown
and wakeup; ordinary client SETTINGS/CONNECT/application IO; local and peer
capsule code/reason preservation and first-close semantics; preservation of a raw
QUIC close cause; and progress past a stalled stream header. The HTTP3 client is
the unchanged `web-transport-quinn` implementation used by fixed relay clients.

The close tests found a real integration requirement: the core's local `close`
starts protocol teardown but does not issue Quinn's application notification.
The owner now explicitly notifies/wakes API waiters on local close and endpoint
shutdown. It also keeps raw transport closure distinct from capsule closure.
Initial compile failures, incorrect client-surface expectations, the peer-close
notification timeout and subsequent corrected receipts are all preserved.

All-target Clippy passes with warnings denied. The full serial suite passes the
eight new tests but fails the native WebSocket origin-budget assertion at
`tests/relay_integration/origin_policy.rs:110` (49152 versus 8192). The identical
assertion was independently reproduced on unchanged retained source. There is no
whole-suite passing claim or claimed fix for that baseline failure. The twelve
retained-backend WebTransport tests pass separately; they do not test the new
backend.

The seven-file patch and complete source archives are frozen and read back;
archive reconstruction verifies all 378 candidate input hashes, and the patch
passes `git apply --check` against retained source. All 374 product inputs and
the selected `91337694` image remain unchanged. No candidate relay binary,
throughput or memory improvement is claimed.

The next required work is ingress/worker sharding, the relay's Send+Sync output
proxy with local stream ownership, exact budgets/admission/metrics/retirement,
and equivalent receive/socket policy. Only then can the fixed-client throughput,
100k memory, datagram, browser and loss/freshness comparisons decide the larger
experiment. The [direct owner progress report](reports/webtransport-owned-protocol-progress-2026-11-07.json)
links the frozen implementation, receipts and concrete remaining integration
contract. The goal remains active; no deployment or product selection changed.

### Direct protocol owner experiment: rejected, 2026-11-07

The integrated experiment now owns UDP/CID ingress and per-worker protocol and
application state directly, retaining quinn-proto, Rustls, ring and the existing
WebTransport parsers. Local stream ownership replaces connection mutexes and
per-connection driver tasks/timer futures. The selected product is unchanged.

Three rotated, serialized 12-second trials per arm/case all pass their healthy
received-work gates. Median verified deliveries/s are:

| Workload | Retained QUIC integration | Direct owner | Change |
| --- | ---: | ---: | ---: |
| Private | 1,508,570 | 1,314,303 | -12.88% |
| Room fanout | 1,707,945 | 1,134,800 | -33.56% |
| Mixed | 1,662,989 | 1,127,640 | -32.19% |

At 100,000 sessions, ready process RSS falls from 1,843.92 to 1,699.98 MiB
(7.81%). Both arms verify 400,000 measured deliveries and clean retirement.
Separate allocation diagnostics show 107,010,342 fewer requested live bytes
(about 102 MiB). Packet/header key and Rustls session construction sites remain
substantial in both arms. Diagnostic heap bytes are not additive to process RSS.

Fresh plaintext WebSocket controls show 1,261,798 / 1,684,481 / 1,644,858
verified deliveries/s for private / room / mixed, and 113.89 MiB ready process RSS
at 100,000 sessions. Those controls were a subsequent block, not interleaved;
WebSocket is plaintext and its kernel TCP memory is excluded from RSS. These
numbers do not establish TLS-equivalent or total-system parity.

The offered-load datagram screen also worsens: median missing fraction rises
from 0.35% to 2.21%; median private arrival-age p99 histogram upper bounds rise
from 33.55 to 134.22 ms. These measure received-update arrival age, not freshest
state age under injected loss. Reliable paced runs deliver all generated work.
Removing redundant worker I/O reactors in a same-binary ablation removes file
descriptors but does not recover throughput. Hardware VAES/AVX2 already executes
in the crypto profile. Relative samples do not prove one dominant library fault.

Targeted existing and experimental checks pass (219 tests, one existing ignored)
and Clippy passes. This is not a full-suite passing claim: the previously
reproduced native origin-budget assertion remains. Final source review found an
additional unfixed experimental adapter gap: a policy close queued during an
input error can be bypassed when the session exits in the same poll, allowing the
outer generic close reason to win. Exact rate/idle/oversize close semantics need
network regression checks and a newly frozen correction before any adoption.
Healthy timing recipes disable rate and idle limits; their verified delivery
counts do not establish general behavior parity.

The experimental performance question is answered negatively. Reject this
prototype; throughput and memory acceptance for the broader pivot remains unmet.
No browser, injected-loss/bulk freshness or deployment acceptance is claimed for
this rejected candidate. Retain protocol/TLS/crypto libraries and investigate
bounded buffer, layout, batching or queue changes only after isolating their
absolute cost. A full handwritten QUIC/TLS/crypto stack is not justified by these
results.

The [complete experiment report](reports/webtransport-direct-owner-experiment-2026-11-07.json)
records identities, performance and allocation metrics, hashed raw receipts,
profiles, failed-launch/controller history, known correctness gaps and the
completion audit. All 374 retained product inputs, 387 candidate inputs and the
selected `91337694` image were read back and verified. Frozen experiment source
and binaries remain separate from the selected product.

### Profiled batching and storage recommendations: completed, 2026-11-07

The follow-up investigation rejects every candidate under the requested joint
throughput, memory and latency gate. Keep the selected `91337694` image and all
374 product source inputs unchanged. Completing these experiments does not
accept the broader transport pivot or establish plaintext-native memory parity.

The final comparison uses the exact selected image, freshly built isolated-target
candidates, the unchanged client, three rotated serialized 12-second repetitions
per workload, and verified received deliveries:

| Workload | Selected deliveries/s | Cold migration storage | Selective owner |
| --- | ---: | ---: | ---: |
| Private | 1,492,353 | 1,504,674 (+0.83%) | 1,305,924 (-12.49%) |
| Room fanout | 1,708,547 | 1,711,632 (+0.18%) | 1,510,520 (-11.59%) |
| Mixed | 1,666,794 | 1,651,827 (-0.90%) | 1,490,965 (-10.55%) |

All 27 runs pass their received-work and cleanup checks. At 100,000 sessions,
ready process RSS is 1,844.67 / 1,803.27 / 1,700.37 MiB respectively; all three
verify 400,000 measured deliveries. Cold migration storage shrinks the protocol
core from 4,656 to 4,224 bytes, but reliable paced private arrival-age p99 median
histogram upper bounds worsen from 4.19 to 8.39 ms. Its initial datagram loss
advantage does not reproduce in the subsequent stock-source control campaign.
The unchanged selected image also varies across cohorts. No stable datagram tail
or loss improvement can be attributed to the storage change.

Same-image causal controls identify useful mechanisms without establishing a
replacement winner. Selective cooperative coalescing improves owner fanout by
36.05% and mixed throughput by 31.42% over its uncoalesced mode. Instrumented
diagnostics show larger UDP submission batches and fewer submissions per verified
delivery. These counters are not exact syscall counts. Disabling retained
synchronous private/control admission costs 5.05% private and 1.38% mixed
throughput; retained fanout already queues. These findings do not fully explain
the residual owner gap. Sampled mutex acquisition spans do not establish a
dominant lock bottleneck. The exact selected user-cycle profile reports no lost
samples and already executes VAES/AVX2 crypto routines.

The experimental queued-policy-close defect is reproduced and corrected before
further measurement, with five actual UDP/TLS/H3 policy checks plus retained
integration checks. The storage candidate passes 323 protocol tests and a real
1 MiB echo before and after client UDP-port rebinding. Those fixes and tests stay
with rejected experiment sources. The small-header variant saves only about
2.55 MiB ordinary RSS at 100,000 sessions and has no consistent throughput gain.

A separate fresh-client TLS1.3 deployment control measures QUIC versus trusted
TLS-fronted WebSocket with the same clients and allowed server CPU set. Median
private / room / mixed deliveries/s are 1,489,098 / 1,727,663 / 1,702,652 for QUIC
and 1,045,029 / 936,689 / 921,769 for TLS-fronted WebSocket. Ready process RSS at
100,000 sessions is 1,844.12 MiB QUIC, 2,570.02 MiB relay plus TLS frontend, and
114.49 MiB plaintext WebSocket. The frontend adds a TCP hop, tasks and copy
buffers; this is a deployment comparison, not an optimized integrated WSS lower
bound. Summed process RSS may double-count shared pages and excludes kernel
socket memory. Kernel SOCK_DIAG queries fail with ENOENT even for known live
sockets, and the installed kernel's inet_diag module is unavailable. Total-system
memory parity remains unverified.

Retain QUIC/TLS/crypto libraries. Consider only bounded changes to measured packet
batching, buffer ownership, connection storage and scheduling. Handwritten SIMD
or a complete protocol rewrite has no measured whole-workload advantage here.
Browser and injected-loss/bulk freshness adoption gates are not run for rejected
candidates. The existing native origin-budget assertion remains outside these
experiments; this is not a full-suite passing claim. No push or deployment occurs.

The [profiled recommendations report](reports/webtransport-profiled-recommendations-2026-11-07.json)
records the requirement audit, current performance and resource metrics, separate
heap/profile diagnostics, 472 hashed evidence receipts, invalid trial exclusions,
and the remaining acceptance gaps. All six original heap files are preserved.

### Configurable reliable receive arena — 2026-11-07

Reliable application records previously allocated a new Vec body for every
message. The optional receive arena now splits writable regions from bounded
worker-local BytesMut slabs. Bytes::from_owner retains the slab reservation
through every published slice; only the final slice can recover a completed
allocation into a single spare-page cache. There is no global pool mutex or new
unsafe code. Existing staging, queue, message-size, validation and timeout limits
remain intact. Exhaustion and records larger than a page use the original path.

`--receive-arena-bytes` caps all arena backing, including cached pages and pages
held by consumers; zero disables it and remains the default. The tested setting
is 8 MiB. `--receive-arena-page-bytes` is the minimum slab allocation on an active
worker, shared across connections; its default and tested setting is 64 KiB.
Allocation is lazy, so this does not reserve a slab for each connection.
The operations guide documents both flags, environment variables and gauges.

The final frozen image is `4429ea34`; on/off comparisons use this same binary,
with the exact previous `91337694` image as an additional control. Three rotated
serialized 12-second repetitions per workload all pass received-work/cleanup:

| Workload | Previous deliveries/s | Arena off | Arena on |
| --- | ---: | ---: | ---: |
| Private | 1,488,869 | 1,482,216 | 1,484,617 |
| Room fanout | 1,707,424 | 1,686,795 | 1,688,083 |
| Mixed | 1,658,450 | 1,635,769 | 1,674,827 |

This is not a general throughput improvement. Same-image mixed throughput rises
2.39%; private and fanout changes are small. Same-image median private server
CPU falls from 2.243 to 2.104 microseconds per verified delivery (6.19%). All
ordinary p99 histogram upper bounds are unchanged. Separate tiny/large-message
checks favor arena-on by 2.03% at 64 bytes and arena-off by 0.66% at 16 KiB;
these are two repetitions per shape, not confidence intervals.

At 100,000 sessions, ready ordinary RSS is 1,843.41 / 1,843.07 / 1,844.62 MiB
for previous/off/on. Each verifies 400,000 measured deliveries with a 1.05 ms
round-trip p99 histogram upper bound and clean resource retirement. The arena
does not establish a resident-memory reduction. In a separate replacement-
allocator capture, ready-to-measured allocation events fall from 3,638,334 to
3,246,123 (10.78%) and requested bytes from 1,458,846,145 to 1,218,375,865
(16.48%). This includes monitoring/background work, not isolated message costs.
An earlier bump-only version reduced request count but not byte traffic; the
completed-page spare is the material reuse improvement.

Compiler layout verifies RecordReader grows from 80 to 96 bytes while the
allocated application task remains 1,792 bytes: existing alignment padding
absorbs the change. This does not remove QUIC's separate receive-assembler
capacity or connection-state footprint. Cached arena backing is reported
separately from staging and can remain warm until the worker runtime exits.

Validation: 174 library tests pass (one existing ignored); 30 integration tests
cover WebTransport, forced transmit retry, read budgets and configuration secrets;
release all-target Clippy passes. Six paced reliable runs deliver all generated
work with zero missing, duplicate or out-of-order deliveries. The feature remains
opt-in because workload-dependent timing and a bounded warm cache do not meet
the broader pivot's joint improvement criterion. No browser, injected-loss,
full native-suite or deployment acceptance is claimed.

The [receive-arena report](reports/webtransport-receive-arena-2026-11-07.json)
records frozen sources, image identities, ordinary/allocator results, size and
paced-load checks, configuration, validation and the completion audit.

## jemalloc and idle-storage reclamation retained — 2026-11-13

Changes on top of `0608124`, each measured against an identically built
baseline with the frozen inspection and capacity clients:

- The relay binary uses jemalloc as its global allocator (non-MSVC targets).
  The QUIC path makes about nine allocations per delivery where the native
  WebSocket path makes almost none, and no allocator had been evaluated.
- Six idle-storage reductions, none of which adds work to steady traffic:
  `poll_endpoint_events` releases its drained queue (192 B per connection);
  the receive assembler starts with one slot instead of four (168 B per stream
  that never queues two chunks); the previous-path option is boxed (432 B
  inline to 8 B); rustls boxes the 0-RTT early secret (80 B to 8 B) and
  releases the drained handshake queue after the handshake (128 B); and the
  Data-space CRYPTO, NEW_CONNECTION_ID and NEW_TOKEN queues are released when
  the frame just sent empties them (480 B).

Verified received deliveries per second, medians of three rotated 12-second
repetitions, with the per-repetition ratio to plaintext WebSocket:

| Workload | Plain WebSocket | WT before | WT, first two storage fixes | WT + jemalloc | jemalloc / WS per repetition |
| --- | ---: | ---: | ---: | ---: | --- |
| Private | 1,190,610 | 1,457,401 | 1,446,673 | 1,500,258 | 1.275, 1.250, 1.282 |
| Room fanout | 1,559,083 | 1,593,495 | 1,651,591 | 1,868,267 | 1.011, 1.165, 1.259 |
| Mixed | 1,298,753 | 1,594,047 | 1,603,103 | 1,726,849 | 1.168, 1.330, 1.223 |

Two further 4-second checks, two repetitions each, compare the later images
with WebSocket: 1.247/1.226 private, 1.236/1.168 room and 1.038/1.229 mixed for
the jemalloc image, then 1.222/1.207, 1.167/1.222 and 1.060/1.411 with the
second storage batch and 1.237/1.234, 1.263/1.335 and 1.289/1.338 for the
committed tree. WebTransport is ahead of the plaintext WebSocket transport in
all 39 paired comparisons of jemalloc images. Median server CPU per verified
delivery falls from 2.28 to 2.00 us private, 1.94 to 1.57 us room and 2.13 to
1.87 us mixed. A preload screen of the unmodified image had already shown the
effect: jemalloc +1.8 to +2.5% private, +14.0 to +17.7% room, +8.3 to +8.9%
mixed in nine paired runs; tcmalloc was similar. mimalloc was built and
rejected: no faster, and 79 MiB more ready RSS.

Ready RSS at 100,000 sessions, all captures passing delivery and cleanup checks:

| Image | Ready RSS, MiB |
| --- | ---: |
| `0608124`, system allocator | 1,838.96 / 1,839.93 |
| + endpoint-event release | 1,821.42 / 1,819.76 |
| + one-slot assembler | 1,769.73 / 1,769.76 |
| + jemalloc | 1,849.80 / 1,853.69 |
| + boxed previous path, boxed early secret, handshake-queue release | 1,715.73 to 1,719.78 (five captures) |
| + pending control-queue release (committed tree) | 1,669.86 / 1,671.81 |

jemalloc's size classes are why the last row matters: it rounded the
5,344-byte connection owner up to 6,144 bytes and the 1,032-byte TLS session
shell up to 1,280. Boxing the previous path takes the owner to the 5,120-byte
class and boxing the early secret takes the shell to 1,024, about 1.3 KiB per
connection together. The 2026-11-07 rejection of an out-of-line previous path
rested on a 0.90% mixed difference and a histogram-bucket change; the saving
is now 2.4 times larger, and the committed tree is within 1.3% of the jemalloc
image on all three workloads in the paired 4-second check (private +1.3%, room
-1.2%, mixed -0.5%). Paced server CPU for one million deliveries falls from
17.84/17.86 s to 15.81/15.10 s with jemalloc. After retirement the process
returns memory (1,334 to 1,402 MiB resident) where glibc kept 1,844 MiB.

An earlier TLS-fronted screen ran its WebSocket arm on the `0608124` image and
its WebTransport arm on a later one, so it is superseded by the same-image
comparison under "Encrypted WebSocket is the baseline" below.

Mixed-workload fairness was examined rather than changed. WebTransport carries
more fanout and less private traffic than WebSocket for a similar total. Per
receiving connection that is a more even service: each of 1,024 fanout readers
gets 1,172 to 1,448 deliveries/s and each of 32 private receivers about 10,000,
against 522 to 696 and about 23,000 on WebSocket. Jain's index over the 1,056
receivers is 0.48 to 0.58 versus 0.09 to 0.11. Shorter scheduler global-queue
intervals (31, 16, 8, 4) were screened as a way to favour private input and
rejected: private throughput did not move and total throughput fell by up to 17%.
A smaller fanout work quantum (256 or 64 recipients instead of 1,024) was
screened for the same purpose and rejected: private throughput and its latency
bound did not improve and fanout fell by 10 to 30%.

Other checks on the committed tree: a 4-second paced run delivers every
datagram and every reliable record in order (p99 arrival-age bounds 2.10 ms
for datagrams, 4.19/8.39 ms private/fanout for streams); plaintext WebSocket
at the same offered load has a 1.05 ms bound and about half the server CPU.
The browser bundle check passes all nine steps in a Firefox 158 engine
(Camoufox), the first non-Chromium run. `scripts/datagram-load.py` now strips
colour codes before reading the certificate pin.

Memory accounting beyond process RSS: per-socket SOCK_DIAG was unavailable, so
host-wide `/proc/meminfo` deltas were taken from before each 100,000-session
run to its ready gate. The plaintext WebSocket run grows kernel slab by 839 MiB
(559 MiB unreclaimable) for 200,000 TCP sockets, 100,000 at each end; the
WebTransport run grows it by nothing measurable. Counting half of that growth
as the server's, plaintext WebSocket costs about 115 MiB of process RSS plus
about 420 MiB of kernel memory, about 535 MiB, against about 1,670 MiB for
WebTransport: about 3.1 times, not the 14 times that process RSS alone
suggests. This is an estimate from host-wide counters, not socket accounting.

Paced load was examined for a simple cause and none was found. Varying the
peer acknowledgement policy (threshold 1 or 7, delay 1 or 5 ms, or none) left
the retained policy with the lowest CPU and no worse latency. A user-cycle
profile of the paced 100,000-session phase has no dominant entry: the largest
self cost is `ConnectionDriver::poll` at 9.4%, followed by packet transmit
and receive, event dispatch, wakeups, timers and crypto at 1 to 4% each. At 60
updates per second without impairment, arrival age is 0.23 ms median for
datagrams, streams and WebSocket alike, with p99 of 1.25, 1.15 and 0.84 ms,
and freshest-state age p99 of 16.6, 16.4 and 17.9 ms.

### Encrypted WebSocket is the baseline

The owner confirmed on 2026-11-13 that the comparison that matters is TLS
WebSocket, not plaintext. The only encrypted WebSocket here is the plaintext
relay behind `examples/tls_front`, a TLS 1.3 terminating proxy on the same CPU
set. `scripts/capacity.py` gained `--transport websocket-tls` for it, and the
fixture gained `--upstream-sources` (so one front end can hold 100,000
upstream connections) and `--copy-buffer-bytes`. Every run in this subsection
uses the committed relay image on all arms.

An independent source audit of the fixture, with a second reader checking each
finding, concluded that it is a defensible model of a proxied wss deployment
and not of a well-built integrated TLS server. Against an integrated server it
handicaps WebSocket in three ways: two 8 KiB copy buffers held per connection
for its lifetime (1.56 GiB at 100,000 sessions, fully resident), an extra
loopback hop with two more copies and four more system calls per message, and
three server-side sockets per session instead of one. Stock rustls also keeps
about 4.7 KiB of idle staging per TCP connection that the vendored patch
releases only for QUIC. In the other direction the fixture is a bare byte
tunnel with none of a real reverse proxy's HTTP handling, timeouts or limits.
Results are therefore reported as a bracket: plaintext WebSocket is the best
case any TLS WebSocket could reach, and the proxy is the measured deployment.

Delivered rate at saturation, two rotated 4-second repetitions per arm:

| Workload | Plaintext WS | Proxied TLS WS | WebTransport | WT / plaintext per repetition | WT / proxied TLS |
| --- | ---: | ---: | ---: | --- | ---: |
| Private | 1,308,839 | 1,016,390 | 1,506,543 | 1.08, 1.22 | 1.48 |
| Room fanout | 1,541,687 | 664,999 | 1,857,127 | 1.18, 1.23 | 2.79 |
| Mixed | 1,337,379 | 765,553 | 1,489,857 | 1.09, 1.14 | 1.95 |

WebTransport is ahead of plaintext WebSocket in all six pairs here and in all
45 pairs recorded in this section. This is a closed-loop rate at fixed
concurrency under an equal CPU allowance, not capacity per CPU: both WebSocket
deployments used 1.7 to 2.4 of the four server cores and WebTransport 2.9 to
3.3. Median server CPU per verified delivery is 1.61 / 1.11 / 1.65 us for
plaintext, 2.34 / 3.06 / 3.15 us for proxied TLS and 1.99 / 1.64 / 2.24 us for
WebTransport (private / room / mixed). The front end's user time is at most
0.32 us per delivery, so an integrated TLS server would be near 1.9 / 1.4 /
2.0 us: WebTransport is 15 to 46% cheaper per delivery than the proxied
deployment and an estimated 3 to 15% dearer than an integrated one.

At 100,000 sessions, 4-second paced phase (two captures of each except the
lean row; all pass delivery and cleanup):

| | Proxied TLS WS, 8 KiB buffers | Proxied TLS WS, 2 KiB buffers | WebTransport |
| --- | ---: | ---: | ---: |
| Deployment process RSS, MiB | 2,577.0 / 2,576.8 | 1,400.9 | 1,671.8 / 1,667.8 |
| Host kernel slab growth, MiB | 1,810 / 1,779 | 1,776 | 0 / -8 |
| Server CPU for 400,000 paced deliveries, s | 4.48 / 4.74 | 4.70 | 5.58 / 5.58 |
| Round-trip p50 / p99 histogram bound, ms | 1.05 / 4.19 | 1.05 / 4.19 | 0.52 / 4.19 |
| Connection setup, s | 4.55 / 4.63 | 4.43 | 10.47 / 10.62 |

Memory. Slab growth is host-wide and covers both ends: the proxied deployment
has 400,000 TCP sockets, 300,000 of them on the server side, so about three
quarters, roughly 1,350 MiB, is the server's. Estimated totals are about 3,930
MiB (8 KiB buffers), 2,730 MiB (2 KiB) and 1,670 MiB (WebTransport). For an
integrated TLS server, which was not built, the audit's arithmetic from these
runs is: the lean front end's 13.2 KiB per connection less 4 KiB of copy
buffers leaves about 870 MiB of TLS and task state, plus 147 MiB of relay and
one socket's share of slab (about 450 MiB), about 1,470 MiB; releasing the
rustls staging would take it to about 1,015 MiB. WebTransport is therefore
well below the proxied deployment and an estimated 14 to 65% above an
integrated one. The plaintext figure of about 535 MiB contains no TLS state.

Paced CPU. Of the proxied deployment's 4.5 to 4.7 s, the relay is 1.5 s and
the front end 3.0 to 3.2 s, of which only 0.6 to 0.7 s is user time; the rest
is the extra hop's system calls. An integrated TLS server would be near 2.1 s,
and plaintext is 1.42 s. WebTransport at 5.6 s is 18 to 25% above the proxied
deployment and an estimated 2.6 times an integrated server.

Latency. At 60 updates per second without impairment, three interleaved
repetitions on one image give arrival-age medians of 0.13 ms plaintext, 0.18
ms proxied TLS, 0.22 ms WebTransport streams and 0.21 ms datagrams, with p99
of 0.18 to 0.20, 0.34 to 0.38, 0.36 to 0.43 and 0.31 to 0.35 ms. An earlier
single-run figure of about 1.2 ms p99 for WebTransport did not reproduce. At
100,000 paced sessions the round-trip histogram bounds are the same bucket at
p99 and one bucket lower for WebTransport at the median.

Connection setup is the least controlled row. The wss clients share one TLS
client configuration with resumption enabled, so many of their handshakes may
be resumed, which distinct real clients would not be; whether the WebTransport
client resumes was not determined. It is recorded, not interpreted.

In short, against the proxied TLS deployment that was measured, WebTransport
delivers 1.5 to 2.8 times the rate, spends less CPU per delivery at
saturation, uses less memory once kernel sockets are counted, and matches it
on latency, at 18 to 25% more CPU under paced load. Against an integrated TLS
WebSocket server, which does not exist here, the delivered rate would still be
ahead, while estimated memory, CPU per delivery and paced CPU would favour
WebSocket by roughly 14 to 65%, 3 to 15% and 2.6 times.

Injected-loss freshness and per-socket kernel accounting could not run until
the host was rebooted onto a kernel with its modules installed; they are in
the next subsection.

Two early passes on this host were disturbed by other local work and by a
baseline built with a different cargo command; they are excluded. One
second-batch capacity run with a 4.2 s client stall is retained and explained
in the report.

Validation: 284 project tests, 327 vendored quinn-proto tests and 248 vendored
rustls tests pass, with all-target Clippy, formatting and the vendor archive
check. Full per-run values, image hashes, method and limits:
[allocator and idle-storage report](reports/webtransport-allocator-idle-storage-2026-11-13.json).

### Kernel sockets and impaired networks, after the reboot

Per-socket kernel accounting at 100,000 sessions (kernel 7.2.15, one run per
transport, same relay image, all pass):

| | Plaintext WS | Proxied TLS WS | WebTransport |
| --- | ---: | ---: | ---: |
| Deployment process RSS, MiB | 147 | 2,577 | 1,668 |
| Server-side sockets | 100,000 | 300,000 | 1 |
| Host slab growth, MiB (both ends) | 957 | 1,890 | -1 |
| Server-side share of slab, MiB | 478 | 1,417 | 0 |
| Process plus server-side kernel, MiB | 626 | 3,994 | 1,668 |

Both TCP runs grow kernel slab by the same 4.8 to 4.9 KiB per socket, about
twice the 2,432-byte socket object, and hold no queued buffer memory at the
gate. This replaces the earlier halved host-wide estimate with a count of the
sockets each process owns and changes little: WebTransport is 2.7 times
plaintext WebSocket in total memory, 0.42 times the proxied TLS deployment,
and an estimated 11 to 60% above an integrated TLS server (about 1,500 MiB,
or 1,040 MiB with its TLS staging released; still not built).

Impaired networks. The freshness fixture ran for the first time on this
branch's images, and its reorder case showed reliable WebTransport streams
taking about 6 s for a 1 MiB transfer. Two things in the fixture distort that
comparison, and two things in the QUIC stack were wrong.

The fixture: loopback's 65,536-byte MTU lets TCP count its window in 64 KiB
segments, so 1 MiB fits inside TCP's initial window while QUIC sends 1.2 KiB
packets. `scripts/freshness-matrix.py --mtu 1500` removes that; the tables
below use it. And netem delays each packet independently, so its jitter and
reorder profiles reorder far more than ordinary paths do. A `jitter` profile
(50 ms each way, 3 ms jitter, no loss) was added because it isolates
reordering from loss.

The stack, found with temporary counters and confirmed by experiment:

1. Under jitter the first 1-RTT packets of a connection overtake the end of
   the handshake. The receiving stack dropped them (its source carried a
   to-do to buffer them), so the sender saw a loss on its first flight, cut
   its window from 12,000 to 8,400 bytes and left slow start for good.
2. The packet-count loss rule, three later packets acknowledged, then read
   ordinary reordering as loss: 41 such declarations for one 1 MiB transfer
   with no real drops.

With the first excluded experimentally, the packet rule alone left the
jittered transfer at 2.9 to 5.4 s; with both gone it took 1.47 s, the same as
with delay and no jitter. The changes:

- The vendored stack holds 1-RTT packets that arrive from the handshake peer
  before the handshake completes, at most 16 packets and 14,720 bytes, and
  processes them when it completes.
- `server_transport_config()` disables the packet threshold, so loss is
  detected by the RFC 9002 time threshold (9/8 of the round trip) alone.
  Under 2% real loss three paired runs show no cost (700/989/624 ms with the
  packet rule, 673/1,010/717 ms without).
- A review of that setting found that timer-declared neighbours of one loss
  event would count as separate bursts in the MTU black-hole detector, so a
  burst now ends only on an ACK-driven pass. This is preventive; it was not
  reproduced.

1 MiB reliable transfer, ms, three repetitions at MTU 1500 (all runs pass
delivery and cleanup):

| Profile | WebTransport stream, before | WebTransport stream, after | TLS WebSocket |
| --- | ---: | ---: | ---: |
| 2% loss, 20 ms | 1,281 / 2,765 / 2,931 | 1,008 / 960 / 1,141 | 2,903 / 3,766 / 3,413 |
| Jitter, 50 ms | 7,225 / 5,814 / 8,270 | 1,448 / 1,455 / 1,473 | 1,296 / 1,298 / 1,389 |
| Reorder, 20 ms | 5,368 / 6,032 / 6,523 | 4,110 / 5,255 / 4,916 | 2,993 / 2,446 / 1,808 |

Datagram state updates stay fresh where reliable delivery cannot: p99
newest-state age is 69 to 109 ms under loss, 168 to 180 ms under jitter and
109 to 150 ms under reorder, against 0.9 to 1.1 s, 1.4 s and 4.0 s for
reliable streams and 2.9 to 3.7 s, 1.2 to 1.3 s and 1.8 to 2.9 s for TLS
WebSocket; 4 to 14 of 240 datagrams were unreceived under loss and reorder,
none under jitter. Without impairment all three arrive within 0.4 ms at p99.

What is left:

- The relay controls its own sending and its own receiving. With a client
  that keeps the stock packet rule (`--client-packet-threshold 3`) the
  jittered transfer still takes 3.5 to 4.7 s, because the upload hop
  collapses in the client.
- Some jittered runs were still about 5 s; the next subsection traces and
  fixes them. Raising the time threshold to 5/4, tried first, did not help and
  made the 2% loss case more variable (775 / 2,783 / 1,486 ms), so it was
  reverted.
- The reorder profile displaces packets by 12 to 28 ms on a 40 ms round trip,
  beyond any fixed time threshold. WebTransport improves there but stays
  about twice as slow as TLS WebSocket for bulk.

Regression checks on the image before the connection-ID fix below: 100,000 sessions at 1,668 MiB and 5.31
s of paced CPU, unchanged; saturating throughput within 1% for private and
room traffic and between -5% and +1% over six paired mixed runs, which is this
host's noise. Three independent reviews with a second reader per finding
found no blocker; their smaller findings are applied. Per-run values are in
the report under `kernel_socket_accounting_100000_sessions`,
`impaired_network_freshness_4s` and `reordering_tolerance_change`.

### Connection-ID retirement overtaken by reordering

Packet tracing of the remaining slow jittered runs found a third mechanism,
in about a third of them, in stream and datagram mode alike. The server sends
its first 1-RTT packets to the client's handshake connection ID. As soon as
the client issues new IDs, quinn switches the server to one of them, to allow
client stateless resets, and retires the old ID in its next packet. When that
packet overtook the earlier ones, the client endpoint dropped the old ID from
its routing table, received the earlier packet for an unknown ID and answered
with a stateless reset, which the server discarded. The server then declared
its first-flight packet lost and left slow start.

The vendored stack now holds that retirement back until every ack-eliciting
packet sent before the switch is acknowledged or declared lost. This is a
change on the relay's side, so it also protects browsers, whose endpoints may
drop a retired ID at once just as quinn's does. Upstream quinn still retires
immediately. Review found a blocker before commit: merging two held-back
ranges without checking that they were adjacent could re-retire IDs, and
with a peer-chosen Retire Prior To and a migration could make the server
allocate an unbounded vector. Merges are now adjacent-only, a peer's Retire
Prior To releases held-back IDs at once (RFC 9000 section 5.1.2), and a test
covers each case.

On the jitter profile (50 ms each way, 3 ms jitter, no loss), 12 of 30 runs
took 4.5 to 5.1 s before the fix; all 24 runs since take 1,358 to 1,469 ms,
against 1,280 to 1,389 ms for TLS WebSocket. Memory at 100,000 sessions is
1,674 MiB (one 32-byte field per connection), and paced CPU and saturating
throughput are within this host's run-to-run range.

### Worker pinning by default and in-place stream reads

Under the historical closed-loop workload (four relay workers, 8,192 idle rooms,
16-message windows) none of the transports saturate the relay: WebTransport used
2.4 to 3.2 of its four cores on private routing and 1.8 on fanout, with every
worker and client thread well below one core. Delivery is set by the round trip,
and pinning shortens the wake-ups in it. Pinned workers raised private delivery
5 to 29% per paired run at equal or lower CPU per delivery, and fanout about 1%.
`--pin-workers` is now `auto` by default. It pins WebTransport workers when the
process may use exactly one CPU per worker, which partitions only the CPUs it was
given; the bare flag still means `on`, and `off` restores the old default. Legacy
WebSocket stays unpinned unless asked.

Call-graph profiling traced the largest buffer reference-count cost to the
relay's 4-byte record-length reads. Quinn's copying read asked the receive
assembler for a chunk no larger than the caller's buffer, which split the shared
packet buffer and dropped the clone after copying. The vendored stack now copies
from the head chunk and advances it in place (`Chunks::read_with`). Reference-count
functions fell from 6.6% to 4.3% of relay samples; delivery was level over five
paired runs and CPU per delivery fell 1 to 4%. Together, the new default delivers
14 to 16% more private traffic than the old one and 0 to 4% more fanout. The
remaining updates sit in packet decode, UDP batch splitting and send-buffer release,
where removing them would need a different buffer design. Details are under
`worker_pinning_default_and_in_place_stream_reads` in the
[report](reports/webtransport-allocator-idle-storage-2026-11-13.json).
