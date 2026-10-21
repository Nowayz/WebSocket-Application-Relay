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
