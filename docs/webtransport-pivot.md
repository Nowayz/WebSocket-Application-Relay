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
