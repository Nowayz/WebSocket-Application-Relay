# Close lifecycle, sustained traffic, and backpressure

## Why these checks were added

The first finite load checks established basic routing and memory behavior. A
longer real-socket workload was needed to exercise repeated close handshakes,
heartbeat traffic, unread recipients, and cleanup under ongoing delivery.

`examples/stress.rs` uses fixed traffic templates and a fixed 64-bucket logarithmic
latency histogram, so retained workload memory is independent of run duration.
Every expected frame at a healthy recipient is compared with its bytes and order. Its 64 distinct
cyclic templates exceed the maximum outstanding window of 32: the next window
does not begin until all current expected frames arrive. Churn instead uses a
16-byte worker ID/counter value, verifies its exact self-message against the
new session ID, requires the exact empty Close acknowledgement and TCP EOF,
and then records latency for the complete lifecycle. Optional source binding
rotates over `127.64.x.y` addresses on an IPv4 loopback `ws://` destination.

### Churn validation correction (2026-10-16)

Earlier frozen stress clients used an eight-byte counter and stopped their
latency clock after the self-echo. Their Close loop accepted any Close payload
and did not wait for TCP EOF after receiving it. It also lacked an explicit
acknowledgement check before stream termination; the underlying codec can still
reject an abrupt EOF as a protocol error.
Historical stress reports retain their original numbers and source/binary
hashes, but their labels about complete close verification were too strong.
Use those latency figures as connect-to-echo measurements and those cycle
counts with the earlier validation limit. The independent integration,
reference and inspection clients have their own Close checks.

The current stress client distinguishes workers in the payload, rejects missing
or mismatched acknowledgements, and waits for EOF after the exact reply. Tests
feed raw missing/wrong/application frames and hold TCP open after a valid Close
to prove these distinctions. The runner cross-checks accepted, joined,
disconnected and welcome/self-delivery totals for every completed churn cycle.
Results from the new contract should be compared using the same new client on
both sides. Frozen historical stress clients still use their earlier contract.

The runner starts a fresh four-worker server for each case and samples server RSS,
CPU, HTTP response time, and relay counters every 250 ms. Cases run sequentially,
without concurrent compilation. It verifies cleanup to zero connections, sessions,
queued bytes, and variable bytes, with only the persistent global channel left.
The current runner also cross-checks verified messages and joins against the
server's independent counters. The three-socket unread case requires exactly one
slow recipient to be evicted. Fanout additionally cross-checks the complete
sent-message/healthy-reader matrix, encoded input bytes, routed value bytes, and
expected disconnect notifications. Its simultaneous teardown is reported
separately because departure notifications can themselves trigger queue limits.

## Two failures reproduced before the fix

**Normal peer close was not acknowledged.** Tungstenite queues a Close reply when
it reads a peer Close. The original Rust path called `WebSocketStream::close`
again, which attempts a fresh write after closing has started and fails with
`SendAfterClosing`. The queued acknowledgement was not flushed before TCP dropped.
The 64-worker churn case therefore failed in its first wave with
`ResetWithoutClosingHandshake`. A focused Rust regression also failed with that
exact error before the fix. This lifecycle follows
[tungstenite's documented read/close contract](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocket.html#method.read).

**A pending write ignored session cancellation.** The unread-recipient case
overflowed its output budget by the 0.276 s sample. Its three-session registry and
roughly 0.9–1.0 MiB of queued output remained until the 10.058 s sample. The write
waited for its five-second deadline, then another close/flush attempt waited for
a second deadline. Healthy delivery continued, but failed-session capacity and
output reservations remained charged for about 9.8 seconds after detection.

The fixes:

- Flush the automatically queued acknowledgement for peer-initiated Close,
  preserving its valid code/reason. Share this lifecycle with the legacy bridge,
  including connections that have not joined yet.
- Use one deadline for an initiated close send plus acknowledgement. Remove
  routing membership and release queued reservations before this handshake.
- Interrupt output on the session close signal or shutdown. If an output future
  has been polled and is cancelled, times out, or fails, drop its transport; do
  not retry a potentially partial frame through another blocking flush. The later
  fanout refinement below preserves closing when the output never started.
- Separate transport loss, write timeout/error, cancelled output, and failed
  close-handshake diagnostics from malformed WAR/CBOR packets.

## Before/after observations

The baseline server was commit `1b975de`; after records contain the close fixes.
Both sides used the same workload behavior: Linux loopback, four server/client
workers, rates disabled for stress, heartbeats enabled, a 1 MiB incoming limit,
and a queue budget of 1 MiB + ten bytes. Binary hashes are retained in the raw
records. These are one run per case, not repeated capacity estimates.
The after records were captured with the fixes still uncommitted: their `revision`
field identifies the base checkout, while their server binary hash identifies
the measured build. New records also include working-tree state and source hashes.

| Case | Before | After |
| --- | --- | --- |
| 64 clients, private 1,024-byte payloads, 30 s | 37,750,096 verified deliveries | 35,328,176 verified deliveries |
| Sustained throughput | 1,258,301 deliveries/s | 1,177,575 deliveries/s |
| Sustained p99 histogram upper bound | 1.049 ms | 1.049 ms |
| Peak server RSS in traffic case | 6.69 MiB | 6.73 MiB |
| 64 reconnect workers, up to 20 s | Failed after first wave: missing Close reply | 1,911,994 reported cycles under the earlier Close validator |
| Reconnect protocol/close failures | Failed workload | Zero |
| Unread recipient, 12 s, 65,536-byte payloads | Gone at 10.058 s sample | Gone at 0.276 s sample |
| Healthy deliveries during unread-client case | 565,437, all verified | 530,408, all verified |
| Maximum sampled management HTTP latency, traffic case | 2.76 ms | 2.72 ms |

The roughly 6.4% sustained-throughput difference is observed, not attributed.
Output cancellation adds work, but the comparison has only one unpinned run per
side; repeats and profiling are needed to distinguish implementation cost from
variation. The stable p99 histogram bucket is an upper bound, not a precise
quantile. No throughput gain is claimed for these reliability fixes.

The unread recipient was detected and removed within the same 250 ms sample after
the fix, so the trace does not resolve its exact eviction latency. A bounded-stream
unit check separately proves a blocked write responds to policy cancellation
within its 200 ms test timeout, instead of its configured 30-second write deadline.
The healthy socket received every expected frame throughout both pressure cases.
Its maximum observed latency in that pressure case was 2.04 ms after the fix; no forced close hit it.

The unread socket eventually reports a transport reset in both versions. A Close
frame cannot be promised through a transport that is blocked mid-frame. The fixed
path releases resources promptly; cooperative native/legacy clients separately
verify clean Close acknowledgements with their exact codes and reasons.

Churn completed 1.91 million **cumulative lifecycles with 64 workers**, not millions
of concurrent connections. Server joined/incoming counters matched the client's
join and verified-message counts exactly. Server RSS ranged from 5.96 to 7.11 MiB
in the after churn samples. Thirty-second/20-second evidence does not prove
multi-hour memory stability or production capacity. The traffic/pressure clients
deliberately drop their completed sockets at teardown; resulting transport or
write-error counters are expected and no longer misclassified as protocol errors.

Raw records:

- [Baseline, including failed churn](benchmarks/2026-10-15-stability-baseline.json)
- [After close/cancellation fixes](benchmarks/2026-10-15-stability-after.json)

Metadata correction: the first raw records carry the CLI's generic
`payload_bytes: 1024` and cyclic-template description for the churn case. Churn
actually sends an eight-byte counter per lifecycle, as described above. The
current reporter labels this correctly; historical outputs are retained unchanged.

## Reproduce

```sh
cargo build --release --bins --example stress
python3 scripts/stability.py --output artifacts/stability.json
# Faster acceptance of the runner and current server:
python3 scripts/stability.py --traffic-secs 2 --churn-secs 2 --slow-secs 2 \
  --clients 8 --output artifacts/stability-smoke.json
```

The finite `load` tool bounds prepared payloads/vectors/latency samples to 512 MiB
and ten million samples. The stress tool caps its prepared templates at 512 MiB
and reuses them with a bounded histogram. These guards cover workload preparation,
not the entire client process, WebSocket buffers, or kernel socket memory.

## Mixed-size fanout and simultaneous peer departure

The shared-buffer forwarding change introduced a new memory-retention contract,
so fixed-size private traffic alone was insufficient acceptance. Fanout mode
prepares one shared set of 64 cyclic templates, runs persistent reader tasks with
fixed histograms, and bounds window acknowledgements by the healthy reader count.
Every reader checks every complete encoded frame before the next window begins.
An optional first-reader delay applies once per window. Optional unread peers
receive their welcome, then stop polling until traffic finishes; their complete
buffered prefix is checked afterward. Latency starts at window publication and
includes sender work, network, validation, and the requested reader delay.

A 128-client / 127-reader mixed-size run delivered correctly for 30 seconds and
kept sampled server RSS below 14 MiB, then failed during simultaneous Close.
The output queues received a burst of disconnect notifications. Capacity signals
could win before an output future was ever polled, but the native connection
treated every cancellation as a possibly partial write and dropped TCP. Clients
therefore received connection resets while trying to finish normal handshakes.
A focused 128-peer test with four queue slots reproduced the reset before the fix.

Output now tracks its first poll with an inline boolean. A cancellation before
that poll permits the normal closing path, while any possibly partial write
still drops its transport immediately. Before initiating a capacity Close on an
untouched stream, the connection polls one already available input item and
preserves a queued peer Close if present. No payload copy, extra queue, blocking
registry I/O, relaxed capacity limit, or dropped-notification policy was added.

The focused test now passes. A bounded duplex-stream test proves that a cancelled
unstarted write preserves a queued peer's exact Close code and reason. The
blocked native vectored-write test still proves immediate policy cancellation
after writing has begun. The complete local suite passes 38 Rust tests and six
JavaScript socket tests, including unchanged legacy-client interoperability.

Three rotating repetitions compared `b2b2b24` with the fixed binary using one
frozen client, 128 clients, eight-message windows, payloads cycling through
8 / 1,024 / 524,288 / 65,536 / 256 bytes, and ten seconds per run. Four server
workers used physical CPUs 0–3; four client workers used 4–7. Heartbeats stayed
enabled, rates were disabled for stress, and no compilation/profiling overlapped
the cases. The old build failed all three teardown checks; the fixed build passed
all three and verified 3,345,688 healthy deliveries in total. Its transport-loss,
write-error, and close-failure counters stayed zero. The input byte and routed
value byte counters independently matched the client's totals.

The fixed runs still recorded 32–58 capacity-close signals during the departure
wave, and 14–33 output cancellations before their first poll. Every healthy
reader received its peer-Close acknowledgement; none received a capacity Close
in those runs. This distinguishes capacity decisions from actual wire outcomes.
Sampled peak RSS was 13,668–13,752 KiB. These are server-only measurements; they
exclude the client and kernel socket buffers. They demonstrate these finite
mixed-size runs, not a general physical-memory bound or sustained capacity limit.

Raw cases, hashes, core topology, phase markers, final counters, and traces are in
[graceful-close-comparison.json](benchmarks/2026-10-15-graceful-close-comparison.json).
The first discovery run is retained in
[mixed-fanout-baseline.json](benchmarks/2026-10-15-mixed-fanout-baseline.json).
Failed runs are kept as failures; their partial traffic phase is not included in
the successful-run timing summary. The comparison CLI's `--record-failures`
continues all candidates but still exits 1 when any candidate fails.

To reproduce mixed fanout and its phase-specific accounting:

```sh
cargo build --release --bins --example stress
python3 scripts/stability.py --modes fanout --clients 128 --fanout-secs 30 \
  --window 8 --payload-sizes 8,1024,524288,65536,256 \
  --unread-clients 1 --reader-delay-ms 5 --output artifacts/mixed-fanout.json
```

The fixed binary also passed a 30-second run with 126 healthy readers, one unread
peer, and a five-millisecond delay on the first healthy reader per window. It
sent 26,944 mixed-size broadcasts and independently verified all **3,394,944**
healthy deliveries plus exactly **126** disconnect notifications for the unread
peer. That peer was absent from both session and connection counts by the first
pressure sample at 0.300 seconds. Its 22 complete buffered frames were verified
before the expected partial-frame transport reset. All healthy reader Close
handshakes completed; there were no write timeouts/errors, transport-disconnect
errors in the server read loop, close failures, or global budget refusals.

Sampled peak server RSS was **16,116 KiB (15.74 MiB)**. RSS ranged from 13,988 to
16,116 KiB in seconds 5–15 and 13,860 to 14,836 KiB in seconds 15–25; the last
interval stayed at 14,836 KiB. This shows no continued RSS growth over this run's
later samples, not a multi-hour leak proof. Logical queued bytes reached about
65 MiB because the budget charges shared bodies once per recipient. Physical RSS
and logical queue reservations intentionally measure different things.

Of 3,235,139,978 routed encoded-value bytes, **99.784%** reused receive storage;
6,977,460 bytes were compacted once before fanout under the existing enlarged-slab
policy. The counters summed exactly to the independently prepared input values.
No additional forwarding copy was introduced by the close fix. The largest
sampled management response was 3.12 ms and final gauges cleaned up. The 58 queue
capacity signals included the unread peer and 57 signals in the departure wave;
28 cancelled outputs had not started, while the unread recipient's blocked
partial frame retained the transport-drop rule. Raw evidence is in
[mixed-fanout-pressure.json](benchmarks/2026-10-15-mixed-fanout-pressure.json).

A separate small-message cost check used the unchanged frozen private-traffic
client, 64 clients, 1,024-byte raw values, window 16, three rotating ten-second
runs per binary, and the same four-worker/core placement. All six runs passed:

| Metric | `b2b2b24` control | Fixed build |
| --- | ---: | ---: |
| Median verified deliveries/s | 1,563,872 | 1,560,637 |
| Run range, deliveries/s | 1,549,590–1,573,177 | 1,545,946–1,604,520 |
| Median server CPU ns/delivery | 1,778 | 1,762 |
| p99 histogram upper bound | 1.049 ms | 1.049 ms |

The median throughput difference was −0.21%, with overlapping ranges. This is a
cost guard, not evidence of a speed improvement. CPU includes setup/teardown;
throughput excludes setup. The fixed client deliberately drops completed sockets
in this private mode, unlike the fanout mode's explicit Close acceptance. Raw
records and traces are in
[graceful-close-small-cost.json](benchmarks/2026-10-15-graceful-close-small-cost.json).

The original three stress modes also passed after the shared client/runner
changes: two-second checks verified 2,595,264 private deliveries, 206,586 complete
connection lifecycles, and 101,942 healthy deliveries beside an unread peer.
See [graceful-close-mode-acceptance.json](benchmarks/2026-10-15-graceful-close-mode-acceptance.json).
The refreshed normal local release then completed 128 simultaneous Node
WebSocket closes with code 1000 and `wasClean: true`, plus three live native/legacy
socket checks. Health/readiness returned 200 and gauges returned to zero;
[graceful-close-live.json](benchmarks/2026-10-15-graceful-close-live.json) records
the observed processes and executable hashes.

### Close after input quota exhaustion

A final Close-order regression exposed a second reset path: both receive loops
checked rate limits before handling a received Close. With a one-message burst,
a pre-join Ping used the quota; the next valid Close was rate-rejected even though
the decoder had already queued its acknowledgement. The attempted fresh Close
then failed and dropped the stream. The regression reproduced
`ResetWithoutClosingHandshake` before the fix.

Both loops now handle Close before rate accounting. A closing peer performs no
further application work, so refusing its cleanup provides no rate protection.
The current test verifies exact code/reason acknowledgement before and after
joining with the quota exhausted, and no false rate-limit/close-failure count.
Data, joins, Ping, and Pong keep their configured limits. The earlier comparisons
above isolate the first-poll fix; final-build revalidation after this Close-order
change is recorded separately below.

Final-build validation repeated three eight-second 128-client fanout cases and
verified **2,679,192** healthy deliveries, with successful simultaneous Close in
every run. It then repeated the 30-second delayed/unread scenario and verified
**3,440,304** deliveries plus all 126 expected disconnect notifications. Peak
sampled RSS was **14,296 KiB (13.96 MiB)**; the unread peer was absent at the
first pressure sample at 0.315 seconds. Of 3,278,291,100 encoded routed-value
bytes, 3,271,220,400 reused receive storage and 7,070,700 were compacted under
the existing policy. One blocked partial output still dropped its transport;
33 cancelled outputs had never started. No write timeout/error, server read-loop
transport error, global budget refusal, or close failure occurred, and all
resource gauges cleaned up. These are finite local checks, not a leak proof or
an estimate for other hosts, networks, or client populations.

Raw final records are in
[close-lifecycle-final-fanout.json](benchmarks/2026-10-15-close-lifecycle-final-fanout.json)
and [close-lifecycle-final-pressure.json](benchmarks/2026-10-15-close-lifecycle-final-pressure.json).
Their executable hashes distinguish this final build from the first-poll-only
stage above. Records were captured before committing: `revision` names the parent
checkout, while executable hashes and source snapshots identify measured builds.

The final small-message guard also used three rotating ten-second cases per
binary and the same frozen 64-client private-traffic workload as before:

| Metric | `b2b2b24` control | Both Close fixes |
| --- | ---: | ---: |
| Median verified deliveries/s | 1,536,783 | 1,582,701 |
| Run range, deliveries/s | 1,525,490–1,554,366 | 1,571,268–1,594,809 |
| Median server CPU ns/delivery | 1,785 | 1,751 |
| p99 histogram upper bound | 1.049 ms | 1.049 ms |

All six cases passed. The recorded median differences are +3.0% throughput and
−1.9% CPU per delivery. The first-poll-only comparison above was nearly flat;
these checks guard against a regression and do not establish a general speed
gain or attribute one to changing Close priority. Source ownership, body sharing,
and the two-slice writer are unchanged. See
[close-lifecycle-final-small-cost.json](benchmarks/2026-10-15-close-lifecycle-final-small-cost.json).

The final quota-aware release was then loaded into the normal local relay and
bridge. All 128 live Node clients again completed with code 1000 and
`wasClean: true`; the three live native/legacy checks passed, health/readiness
returned 200, and gauges returned to zero. The live snapshot linked above now
identifies this final executable, rather than the earlier first-poll-only stage.

## Small application limits and real browser acceptance

A real-socket regression with `--max-message-bytes 17` reproduced an abrupt
pre-join reset for a valid 125-byte control payload. The WebSocket frame cap had
been set to the application-message limit. It now permits at least 125 bytes;
the independent data-message limit still applies to complete and reassembled
fragmented input. The boundary tests pass in debug and release: 125-byte
Ping/Pong and Close succeed before/after join, 17-byte application data is
delivered, and 18-byte complete or fragmented data closes with `1009`.

The standalone ES2022 client bundle was then loaded in actual Chrome 155 on
Linux. Its seven acceptance checks cover Unicode, byte strings, nested values,
an exact uint64, channel/private routing, a 128 KiB value, concurrent variable
requests, precision checks, disconnect notification, and three clean Close
handshakes. An invalid URL displayed a failure, re-enabled the button, and a
subsequent run passed. Desktop and 390-pixel-wide layouts were inspected; the
console reported zero errors/warnings. This verifies this browser and generated
module. Other engines and production app bundlers remain future work.

The isolated browser run reused 131,173 encoded value bytes, copied none, added
no transport/protocol/write/Close errors, and returned session, connection,
queue, and variable gauges to zero. A separate release smoke checked all 12,700
64 KiB broadcast deliveries across 128 sessions. That finite helper drops
socket handles at completion, so its 76 transport-disconnect counter increments
are retained in the record; its timings are not a performance comparison.
The shared application-buffer ownership and vectored writer are unchanged.

One local bridge process exited with code zero and no error output during
recording. Its cause has not been reproduced or established as a code defect.
The replacement passed three live checks covering the unchanged original
client, mixed CBOR/legacy bytes, and native/legacy Close before/after join.
The record identifies the replacement process and matching executable hash.

See [browser-control-acceptance.json](benchmarks/2026-10-15-browser-control-acceptance.json)
for source/binary hashes, metrics before/after browser execution, the page's
results, and verification limits. The README includes commands to rebuild and
serve the same acceptance page. CI builds its bundle; browser execution was
performed locally with the Playwright CLI.

## Reserved CBOR simple-value encodings

The deterministic malformed-input campaign found a decoder permissiveness gap:
`f8 00` passed validation and reached a healthy recipient. The same vector failed
the new unit check and real-socket isolation regression before the fix.
[RFC 8949 section 3.3](https://www.rfc-editor.org/rfc/rfc8949.html#section-3.3)
excludes `f8` followed by a byte below 32. The relay now checks that condition
only in its simple-value branch, retaining the fixed validation stack, borrowed
payload storage, and existing integer/float/tag behavior.

The network regression rejects 192 cases: all 32 reserved expansions in direct,
array, map-value, tag, map-key, and set-variable positions. Eighty routed cases
split the final CBOR byte into a continuation frame with an interleaved Ping.
The healthy peer verifies 160 self-deliveries between rejections; malformed
values cannot reach it or populate a variable. All 248 valid simple encodings
(224 expanded and 24 one-byte forms) are then forwarded byte for byte. Both
healthy sessions finish clean Close handshakes. The case passes in debug and
release. The full suite now passes 43 Rust tests, strict Clippy, formatting, and
six JavaScript socket checks.

Independent unit vectors also test all 1,280 expanded-simple placements, every
truncation of selected wire values, trailing items, reserved additional
information, malformed UTF-8, duplicate map keys, nonminimal definite headers,
opaque tags, extreme integers, floating-point payload bits, 64/65 nested
array/tag levels, and the 65,536-item boundary. This is a deterministic corpus,
not exhaustive generated fuzzing or a claim that every malformed input has been
tested. Valid integer encodings remain accepted without canonicalization.

The normal local native/bridge releases were refreshed afterward. Live routed
`f8 00` and set-variable `f8 1f` cases each closed cleanly with `1002`; the
recipient stayed usable and no variable was stored. The seven-check Chrome
acceptance and three live legacy/Close checks passed against these processes.
Health/readiness returned 200, session/connection/queue/variable gauges returned
to zero, and unexpected transport/write/Close errors stayed zero. The two
protocol-error increments are the intentional malformed probes. See
[cbor-retention-live.json](benchmarks/2026-10-15-cbor-retention-live.json)
for executable/source hashes, live results, counters, and verification limits.

The separate retained-output campaign passed all six cases with no policy
change. Its full method and memory measurements are recorded in
[performance.md](performance.md#retention-under-repeated-unknown-target-primers).

## Application-idle retirement and shared deadline scheduling

An optional `--idle-timeout-secs` policy retires joined sessions after application
inactivity, using `1001` / `Application Idle`. It is disabled by default. Valid
binary input and written relay output refresh activity; Ping/Pong does not.
Queued/in-flight output keeps the existing write/queue policy. Read-only
consumers remain active while data arrives, and incomplete WebSocket data
messages do not indefinitely refresh the application clock.

Three network regressions exercise control traffic without application activity,
ongoing input plus receive-only output beyond one timeout, and a deliberately
unfinished 1 MiB fragmented message. Expiry completes normal Close cleanup,
including ordinary channel-variable release on last-member departure. The
control case verifies an 18-byte idle Close acknowledgement while incoming
application messages are limited to 17 bytes. The existing no-pong test now runs
with idle expiry both disabled and set to ten seconds, proving the two-second
heartbeat deadline preempts the later idle alarm. The full suite passes 46 Rust
tests; the updated JS fixture passes six default cases plus one configured-idle
case covering both native and legacy WebSocket APIs.

The first implementation reused the timeout wait but retimed it on every active
message. Its enabled profile attributed 8.38% of sampled user cycles to
`Sleep::reset`. The final implementation keeps an already earlier alarm when
activity moves expiry later. A completed old alarm checks the live deadline and
rearms before another poll. Earlier heartbeat deadlines still retime immediately;
the change preserves absolute expiry without coarse periodic checks. No
application-body movement, queue-budget change, or transport-cancellation change
was introduced.

All twelve initial multi-producer retention cases and three final cases passed
full delivery/ordering, counter, idle-policy, Close, and cleanup checks. The final
64-producer case verified 4,105 data/barrier deliveries, 65 clean server-idle
handshakes, and zero unexpected transport/write/Close errors. Before acknowledgement,
65 transports retained 269,212 KiB sampled RSS despite drained output. After
retirement RSS was 6,560 KiB and connection/session/queue/variable gauges were zero.
The large routed values reused 8,388,648 encoded bytes; 5,249 tiny encoded bytes
were compacted. This is finite ownership/retention evidence, not a leak proof or
a physical-memory bound for active connections. The full memory/cost methods and
records are in [performance.md](performance.md#concurrent-producers-connected-idle-storage-and-retirement).

The final local releases were refreshed with the application-idle policy
explicitly disabled on the normal test endpoint. Chrome's seven-check acceptance
and three live original-client/native-legacy/Close checks passed. Health and
readiness returned 200, gauges returned to zero, and protocol/transport/write/
Close error counters stayed zero. The enabled-policy evidence comes from the
separate fresh-server campaigns and configured-idle JS fixture. See
[application-idle-live.json](benchmarks/2026-10-15-application-idle-live.json)
for process/source/binary hashes and exact verification boundaries. No public
deployment or CI execution is claimed.

## Rejections before join and mixed input, 2026-10-16

A socket regression found that a successful HTTP upgrade followed by an
unmasked frame, invalid fragmented control frame or oversized data header could
end with a TCP reset before channel join. The same failures already sent policy
Close frames after joining. Native input-error mapping is now shared across
both phases: `1002` for protocol errors, `1009` for excessive size and `1013`
for staging exhaustion. Header-based rejections do not wait for a refused body.
The new `war_join_rejected_total` counter records pre-join rejection decisions;
normal peer Close, HTTP admission and shutdown retain their separate behavior.
Existing joined-session counters and shared application-body ownership remain
unchanged.

The [raw-client corpus](../scripts/input-corpus.py) builds CBOR and WebSocket
input independently of the Rust codecs. Its 776 input/context cases become
2,311 exchanges across whole, fragmented and pipelined forms:

- Nineteen accepted values cover nonpreferred integer/string lengths, full-width
  integers, opaque tags, duplicate map keys, floating-point bit patterns, and
  exact depth/item limits. Each returned encoded value is checked byte for byte.
- 243 rejected encodings cover truncation, trailing data, reserved simple values
  and additional information, malformed UTF-8, impossible lengths and exceeded
  depth/item limits. Each runs as a routed value, a different-target value and
  an attempted variable update. Indefinite forms excluded by WAR are identified
  separately from malformed CBOR.
- Invalid joins and raw WebSocket cases run before and after registration.
  Fragmented messages mix nonempty chunk sizes with 257 empty continuations and
  an exact interleaved Ping/Pong. Raw bad frames are also written one byte at a
  time; this tests split writes without assuming TCP packet boundaries.
- An ordered witness self-message follows each exchange, after checking the
  exact expected disconnect notification. This proves invalid data was not
  queued ahead of the barrier. Variable sentinels must survive rejected updates.
  Pipelined invalid input may reject before the queued welcome is sent; joined
  and delivery counters are checked against the observed wire events.

The control and fixed smoke runs reproduce 18 previously silent pre-join drops
and verify all 18 new Close responses. A passing control record describes that
expected defect, not correct acceptance. Full runs with one worker/one producer,
four workers/eight producers, and the default README command all pass. Every full
run verifies 2,204 complete-message policy Close handshakes, 45 terminal-decoder
Close receipts, five library-normalized peer-Close replies and all ordinary
Close handshakes. Terminal failures do not claim peer acknowledgement. The
normalized Close cases already worked in the control and are excluded from the
new application rejection counter. Exact rejection/message/byte/delivery totals
match; final staging/connection/session/queue/variable gauges are zero, with no
unexpected write, Close, transport, handshake or admission errors. Routed-value
compaction remains zero. Independent 1 KiB self-routing stays verified throughout.

Focused socket checks additionally hold incomplete binary messages across 513
empty continuations and an interleaved Pong. The join deadline still fires while
75 healthy round trips continue; its staging lease retires. Admin cancellation
removes membership and queued reservations before waiting for Close, while
healthy routing continues. Shutdown closes both pre-join and joined fragmented
sessions without incrementing join rejections; all resource guards release.
Rate and staging rejections count their stage exactly once, while HTTP `503`
admission does not increment the new counter.

All 73 Rust tests, Clippy, formatting, the 62-file vendor audit, seven JavaScript
socket cases and 48-common-case/eight-explicit-difference original C++ comparison
pass. [The build manifest](benchmarks/2026-10-16-input-rejection-build.json) binds
source, package, patch, verifier and release hashes; the
[summary](benchmarks/2026-10-16-input-rejection-summary.json) binds every raw
record and its exact counters. Ordinary traffic cost is recorded separately in
`performance.md`. This finite corpus does not establish exhaustive fuzz coverage,
production capacity or arbitrary network/teardown behavior.

## Optional browser-origin admission, 2026-10-16

Native and legacy listeners now share one optional, exact Origin allowlist.
The default remains unrestricted. With a configured list, an unlisted, empty,
combined or duplicate Origin receives HTTP `403` before upgrade. Origin-less
clients remain supported, and `null` requires an explicit entry. The policy
runs once during the handshake, borrowing headers and bounded configuration;
application validation, shared bodies and routing scopes remain unchanged.
The bridge checks its own public ingress, since its upstream has no browser
Origin. Its HTTP reader now shares the native 16 KiB raw upgrade guard.

Four real-socket regressions cover exact/IPv6/missing/explicit-null origins,
unrestricted defaults, unsupported subprotocols, malformed and duplicate Origin
values, exact `403` body/length, and native counter scopes. A healthy session
continues routing through repeated denials with only two connection slots.
Each denial releases its slot and decoder allowance; it never registers a
channel member or changes queued bytes. The bridge test uses one slot, verifies
that denial and oversized HTTP input create no upstream connection, and then
routes a 64 KiB frame after a valid upgrade. Accepted peers finish normal Close
and EOF. Three unit checks cover bounded configuration, exact borrowed header
comparisons and repeated/comma-separated CLI arguments.

Headed Chrome 155 exercised the actual generated browser module with the
allowlist set through `WAR_ALLOWED_ORIGINS` on both binaries. The allowed page
at `http://127.0.0.1:4180` passed seven native checks, including typed/128 KiB
delivery, control serialization, precise IDs, disconnect and clean Close.
A browser legacy connection preserved 64 KiB binary and Unicode text messages
and closed cleanly. Serving the same page at port `4181` caused actual HTTP
`403` upgrade failures on both listeners. The native denial incremented the
Origin and general rejection counters once, without a handshake/join error;
the bridge denial created no native connection. All resource gauges returned
to zero. The browser's two expected error/`1006` observations are recorded as
denied upgrades, rather than clean WebSocket Close handshakes.

All 80 Rust tests pass in debug, with the new policy checks also passing in
release. Strict Clippy, formatting, 62-file vendor verification, seven JS socket
cases, the independent 776-case/2,311-exchange input corpus and the original C++
48-common-case/eight-explicit-difference comparison pass. This preserves the
finite prior protocol contract with the policy disabled. Evidence is bound in
[the manifest](benchmarks/2026-10-16-origin-build.json), with
[browser observations](benchmarks/2026-10-16-origin-browser.json),
[input corpus](benchmarks/2026-10-16-origin-corpus.json), and
[original-runtime comparison](benchmarks/2026-10-16-origin-reference.json).
The [operations guide](operations.md#browser-origin-admission) documents the
proxy and authentication boundary. Other browser engines and a deployed TLS
proxy are outside these checks.

## ID registry lifetime across large populations, 2026-10-16

Global routing now reduces excess ID-table capacity when IDs retire, through
either session removal or occupied fixed-ID takeover. The testable invariant
is that this moves table metadata only. Registered peers and published payloads
must retain their owners, addresses and byte reservations; a displaced session
must never remove its replacement's ID during later cleanup.

Two unit regressions exercise 1,024 transient owners. One holds an in-flight
encoded value while the table shrinks repeatedly, checks peer and payload
pointer identity, verifies later private delivery, then drops both peers while
the held frame still owns its reservation. The other makes 1,023 consecutive
takeovers while all displaced registrations remain alive, verifies the final
owner and reduced table capacity, then retires every registration. That second
case fails against the initial departure-only prototype; both retirement paths
use the shared helper in the selected implementation.

A four-worker socket regression keeps private delivery active during 2,048
arrivals and departures, with 32 concurrent client handshakes/Close operations.
Every healthy value remains byte-identical, progress occurs during both phases,
all transient peers receive normal Close acknowledgements and EOF, and the two
original peers remain usable. Resource guards return to zero at shutdown.

The new `registry_peak` example and `scripts/registry-peak.py` additionally keep
the same two global clients connected through population waves. Transient peers
use separate ordinary rooms and no global disconnect listener, isolating ID-map
history from departure fanout. Every data frame, sender, byte, FIFO position,
Close code/reason/EOF and final disconnect is verified. Gated phase boundaries
let the runner compare exact message/byte/delivery counts and separate native
routing CPU from filling/draining. All final session, connection, queue, variable
and staging gauges must be zero, with only the persistent global channel.
The selected comparison verifies 31,309,200 deliveries and 147,468 normal Close
handshakes across six processes, three 8,192-peer waves per process. Profiles
and prototype measurements remain distinct from selected timing evidence.

The runtime change preserves routing, listener, variable, ID and CBOR contracts.
The [performance record](performance.md#id-registry-capacity-after-population-waves-2026-10-16)
documents cost, allocation tradeoffs and finite-workload limits. Other registry
tables and much larger active fanout remain outside this optimization.

The runner also validates service exit after its data and resource checks.
A calibration wrapper serves real sockets and closes its relay child cleanly,
then deliberately exits with status 7. This reproduced a false success in the
initial timing runner; the corrected runner returns failure and CLI status 1.
A normal-service smoke case passes. All recorded population-comparison services independently
have exit status zero; their exact earlier runner snapshot is retained for
provenance. Calibration records are excluded from performance results.

## Membership lifetime during room departures, 2026-10-16

The same bounded metadata helper now handles native ID ownership and room
membership, including the persistent global room. An empty ordinary room still
retires the complete channel and its variables. A room with retained peers
can release spare member slots; an empty global room preserves its variables.
Only keys and `Arc` handles move. No application-message path changes.

A unit regression first fails on retained membership capacity in the prior
release. It then verifies ordinary and global rooms with 1,024 administrative
close signals: exact disconnect events to the healthy peers, peer pointer
identity, variable value/pointer identity, later private delivery, and a held
application frame across rehashing and final peer destruction. Its byte
reservation lasts until the frame drops. Ordinary variables release at the
last departure; global variables keep their documented lifetime.

The population probe now supports `--scope room` and `--scope private`, keeping
transient peers in the same ordinary or global room as the two original clients.
All departing IDs are preencoded once; every remaining client checks the exact
FIFO prefix. Sixteen-close windows wait for every remaining reader before more
closes begin, keeping the default 64-message queues and shared byte limits.
No departure notifications are suppressed. At 8,192 transient peers this verifies
33,566,720 departure deliveries per process, alongside exact normal Close/EOF,
timed application traffic, metric totals and final resource release.

Broadcast scope exercises the ordinary membership index; private scope uses
global-room members with zero listener flags to isolate that index's scan from
extra listener fanout. Profiles, ordinary traffic guards and the
[membership performance record](performance.md#membership-index-capacity-after-room-departures-2026-10-16)
retain their separate scopes. The original `global` probe remains the default,
with transient peers in separate rooms.

## Channel-name lifetime through room waves, 2026-10-16

The same bounded metadata policy now covers the native channel-name registry.
The check runs only when an ordinary room's final member leaves and its channel
entry is removed. The persistent global room and its variable lifetime remain
intact. Sorting, authentication, resource admission and routing behavior retain
their existing contracts.

A unit regression first fails on retained capacity before this change. It keeps
binary, ASCII and global rooms through 1,024 other rooms, then checks channel-name
and variable byte pointers, peer identity and a held 8 KiB application frame.
Independent literal CBOR verifies raw-byte channel ordering; independent JSON
verifies the management endpoint's distinct base64 ordering, including a
non-UTF-8 name. Retained variables survive, ordinary variables are absent on
rejoin and global variables persist. The held frame retains its byte reservation
and payload address through final peer destruction until the envelope drops.

The population probe's `--scope channels` checks every CBOR reply and every field
of authenticated HTTP inspection at each fresh/peak/retired/empty gate. Six final
processes verify 33,165,328 queries and 98,316 exact normal Close/EOF handshakes
through two waves of 8,192 separate rooms each, with exact metric counts/bytes
and complete resource release. Existing concurrent private routing also passes
through 2,048 arrivals and departures. Large active inspection lock contention
has not been benchmarked.

The ordinary stability runner now publishes its verdict after waiting for the
server process and records exit status, forced-kill status and final logs.
A real-socket calibration wrapper reproduces the old false success by exiting
7 after a native child has exited zero; the final runner rejects it. Every
ordinary cost case exits zero. A rejected diagnostic recording exposed an
overly strict `perf` stop-status check; the corrected runner accepts exit zero
or the SIGINT it deliberately sent, and a fresh complete recording passes.
Failed/oracle observations are retained separately from successful timings.

Updated manual services keep binary/ASCII/global rooms through 1,024 distinct
ordinary rooms, verify 65 typed private deliveries during/after close windows,
1,029 clean closes, exact incoming/output byte counts and zero extra body copies.
Ordinary variables survive retirement then disappear on rejoin. Default decoder
reclamation, fresh browser acceptance and three named legacy/Close cases pass.
The [channel-name performance record](performance.md#channel-name-capacity-after-room-waves-2026-10-16)
states measured costs and limits; these remain finite local checks.

## Snapshot lifetime during concurrent inspection, 2026-10-16

Channel inspection now owns bounded names and numeric metadata captured under
one read lock. Formatting, sorting, CBOR encoding and enqueueing occur after
release. CBOR and HTTP retain their distinct raw-byte/base64 order and existing
authentication/schema. A snapshot can reflect an earlier instant than the
registry state at delivery; it is coherent at capture and has no cache.

The new regression holds both snapshot forms while acquiring the registry write
lock, joining another peer, replacing a variable and destroying all peers. The
captured counts/name bytes stay valid and unchanged; no peer or variable value
is retained. An already routed 8 KiB body keeps its original pointer and byte
reservation through all those operations, then releases it when its envelope
drops. Normal forwarding retains shared body storage.

Eighteen final processes keep 8,192 stable rooms, two private pairs and eight
reconnect workers through eight measured seconds each. The no-inspection,
authenticated CBOR and authenticated HTTP cases verify every delivered value,
sender/order, channel name/count, HTTP metadata field and normal Close/EOF.
Only known one-member churn rooms may appear during the timed concurrent view;
exact quiescent snapshots bracket the workload. Binary, NUL and Unicode channel
names are included. All 71,136,608 healthy deliveries, 6,832,194 reconnect
lifecycles, 19,200 timed inspections and 6,979,734 normal closes pass. Exact
counter/byte totals, zero routed-body compaction, final resource release and
client/server exit zero are checked independently at the gates. This is finite
loopback evidence with default queues, rates/heartbeats disabled, and separately
recorded latency/CPU tradeoffs in the
[inspection measurement](performance.md#concurrent-channel-inspection-2026-10-16).

## Terminal shutdown admission, 2026-10-16

A management regression reproduced a shutdown being resumed: after shutdown
set the former separate drain/stopping atomics, an authorized resume returned
`204`, clearing the drain observed by readiness and admission. The regression
expected `409` and failed before the fix. This was a deterministic local handler
reproduction; it does not establish that the short CLI signal race occurred in
production.

One private atomic lifecycle now represents open, draining and stopping.
Drain/resume uses compare-and-update and cannot overwrite stopping. Redundant
drain remains successful; authorized resume after stopping returns `409`.
Readiness remains `503`, metrics report `war_draining 1`, and fresh connections
and pending joins remain refused. The public Rust atomic field is replaced by
checked methods; see [the embedding migration](operations.md#drain-and-restart).
Forwarding, shared application-body ownership and recipient queues are unchanged.

Four focused checks cover the reproduced handler failure, reversible/terminal
transitions, concurrent management changes versus shutdown, and a real HTTP
management listener with four native workers. The socket check keeps an existing
session usable during manual drain, verifies normal resume, then publishes
shutdown before worker notification. Unauthorized resume is `401`, authorized
resume is `409`, a pending join receives `1013` and EOF, and a fresh transport
closes without admission. Cancelling the data service produces `1001` for the
joined peer and releases its resources. A separately hosted management listener
still refuses resume after the data service has ended.

The [checkpoint evidence](benchmarks/2026-10-16-shutdown-state-build.json) binds
127 Rust tests, strict Clippy/formatting and release builds to the source and
frozen binaries. It also binds C++ compatibility, JavaScript, API, socket stress
and real browser checks. The [routing comparison](performance.md#terminal-shutdown-admission-2026-10-16)
retains the measured throughput costs and supplemental check. These finite
local checks do not establish long-term or hosted behavior.

## Accept recovery under descriptor pressure, 2026-10-16

The preceding release propagated every non-interrupted/non-WouldBlock native
accept error out of its worker, causing service shutdown. The bridge propagated
accept errors too. The [before-fix oracle](benchmarks/2026-10-16-fd-pressure-oracle-final.json)
runs each ingress with a child-only 64-descriptor limit. One/four native workers
and the legacy bridge each exit with `EMFILE` and drop an established client.
The OS limit and an independently retained management connection distinguish
this failure from a relay quota or malformed WebSocket input.

The fix retries accept failures while preserving established connections.
Incoming connection reset/abort/refusal errors retry in the existing bounded
schedule; other accept errors wait one second, following the management
listener's existing portable policy. Native workers suspend listener readiness,
continue socket work/timers and explicitly drain already queued flows on retry.
The bridge retains its tasks and cancellation while waiting. Native
`war_accept_errors_total` exposes the failure independently of policy refusals.
No message-forwarding or shared-body ownership changes.

The same independent masked-frame probe passes on
[final debug binaries](benchmarks/2026-10-16-fd-pressure-debug-final.json) and
[release binaries](benchmarks/2026-10-16-fd-pressure-acceptance.json). Each case
reaches exactly 64 open descriptors, observes the accept failure, checks exact
private echoes at capacity and recovers a connection queued before capacity
returns. The two clients use distinct rooms so a valid departure notification
cannot be mistaken for a private echo. All retired pressure sockets release;
only the original session remains before the second wave. That second wave
checks shutdown during backoff, including native `1001` and process exit zero.
Linux CI runs the same final probe against its debug binaries.

The release queued-flow recovery takes 0.398–0.400 seconds after capacity is
freed about 0.6 seconds into the retry interval. The CPU counter does not advance
at its 100 Hz resolution during those approximately 0.6-second pressure samples;
this is an observation limit, not proof of zero CPU work. Shutdown takes
1.2–3.3 ms in these runs. The test limits only disposable processes and does not
simulate system-wide exhaustion or allocator failure. New management TCP
connections also need descriptor capacity; the probe keeps one HTTP connection
open before pressure. The [operating contract](operations.md#accepting-under-os-resource-pressure)
and [ordinary performance comparison](performance.md#accept-recovery-under-descriptor-pressure-2026-10-16)
state the scope and measured costs.

## Large-room broadcast scheduling and ownership, 2026-10-16

A large-room probe now verifies every reader's full encoded body, sender and
FIFO, a complete reader/message matrix, and exact controlled departure events.
It keeps one private pair and eight reconnect workers active concurrently.
A fake server corrupts a non-primary reader while keeping the primary transport
open; the probe rejects the corruption. All real phases independently match
native metric/byte totals, normal Close/EOF and final resource release.

The old read quota counted only input frames, allowing up to 256 complete
broadcasts in one connection turn. A deterministic regression preloads two
masked messages for 300 recipients and fails before the scheduling fix. It now
verifies one complete fanout before yielding, then the next complete fanout in
FIFO order, with no queued-byte leak.

Ordinary broadcasts without global channel observers now capture a shared
recipient view and release the registry before output. Stable rooms reuse the
same view; joining/leaving invalidates it. Tests acquire the write lock inside
an output callback, verify view reuse/replacement and captured peer ownership,
and keep the exact original 8 KiB body pointer and byte reservation through
membership changes and final room destruction. The cache holds peer handles
and no message values. Global broadcasts and observer paths keep their existing
submission behavior.

Thirty placement-controlled before/after processes plus six pure-private guards
pass all checks. Source placement is verified from kernel socket ownership
before traffic; setup-only rejects are retained separately. At 512 readers,
reconnect p99 falls from 67.109 to 8.389 ms buckets. Fanout throughput falls
about 3–6%, with private/admission progress improving substantially. Separate
heap diagnostics measure 256 KiB extra channel metadata at 8,192 idle rooms
and 4,144 bytes/two allocations for the warmed 513-member view. The
[performance record](performance.md#large-room-fanout-fairness-2026-10-16)
retains exact scopes, ranges, rejected experiments, allocation limits and the
normal-placement reproduction command. These checks do not establish a hard
time bound or the same improvement for global/observer traffic or frequent
membership changes inside the busy room.

## Accepted socket setup failures, 2026-10-16

Runtime `9a941637c63f5bd34a82c27db985cdeba2388c31` isolates failures after
accepting native TCP. Previously, `TCP_NODELAY` and readiness-registration
errors propagated out of the worker and stopped the service. The independent
[before probe](benchmarks/2026-10-16-setup-pressure-before.json) injects
`ENOBUFS`/`ENOMEM` into those specific syscalls in separate one/four-worker
processes. All four baseline processes exit 1. The probe also holds the failed
descriptor open: the TCP-configuration path had already released its connection
slot, reporting one connection while two data descriptors remained open.
Registration failure already preserved that ownership order.

Setup errors now retire the affected socket, count `war_setup_errors_total` and
`war_rejected_total`, and reuse the worker's one-second listener suspension.
Established socket work, timers and shutdown continue. Queued connects are
explicitly accepted when the retry expires. Other workers can still accept.
A local stream binding declares descriptor ownership after the admission guard,
so reverse local drop order closes TCP first on every early setup refusal.
After registration, the existing `Socket` field order provides the same rule.
Routing, payload representation and buffer ownership are unchanged.

The identical final probe passes on [release](benchmarks/2026-10-16-setup-pressure-after.json)
and [debug](benchmarks/2026-10-16-setup-pressure-debug.json) binaries. Across eight
cases it injects 24 setup failures: two recovery waves and a third shutdown wave
per case. Every close gate observes two charged slots, each recovered client
verifies its welcome/self-echo and exact Close/EOF, and an established client
continues exact private echoes. Admission/join/retirement/error counters match;
queues and variables are empty, and decoder allowance returns to the established
client's initial level after recovery. Shutdown delivers `1001`, reaches EOF
and exits zero during retry. It flushes Close and retires TCP without waiting
for an acknowledgement; an initial probe that sent a late acknowledgement
sometimes induced a reset and was corrected to exercise that existing contract.

Single-worker release recovery takes 0.980–1.001 seconds after the failed
socket retires. Four-worker queued clients in this sample reach another worker
in under a millisecond; that is observed placement, not a recovery deadline.
Release shutdown takes 1.1–1.2 ms in these finite cases. The existing
[descriptor-exhaustion probe](benchmarks/2026-10-16-setup-pressure-accept.json)
also passes one/four native workers and the legacy bridge after sharing the
listener suspension helper. These checks use child-only fault injection or FD
limits, and do not establish behavior under system-wide memory exhaustion.

## Reusable recipient views under membership churn, 2026-10-16

Runtime `0f70200` updates a broadcast recipient view in place only while the
registry is exclusively locked and the view has no routing owner. Otherwise a
membership change invalidates the cache and existing captures finish unchanged.
The cache holds peer handles and no application values. Removal releases its
handle immediately, and sparse capacity shrinks after a large wave; an empty
ordinary room drops the view. No new index, per-peer field or synchronization
object is required.

The new reuse/retirement tests fail on the baseline and pass on the selected
implementation. They verify that uncaptured membership updates preserve the
view allocation, welcome/new-body delivery reaches the added member, departure
releases its last cached owner, an older retained frame keeps its original
bytes/reservation, and final cleanup releases everything. A 1,024-member wave
leaves at most 256 slots while two members remain. Existing tests still verify
in-flight view invalidation, captured-owner lifetime, original 8 KiB payload
pointers and output after releasing the registry lock. The complete suite has
134 passing Rust tests.

Helper `b01b166` joins and retires a fully verifying temporary reader inside the
broadcast room. Its controller gates membership changes between complete
windows, checks every stable/transient body and FIFO, completes exact Close/EOF,
and checks the unique departure at every survivor including the sender.
Independent service counters verify all bytes/messages/joins/closes and final
resource retirement. The negative departure oracle keeps transports open and
changes a notification only at a secondary reader; validation rejects it there.
The existing secondary-body corruption oracle continues to pass.

The [ordinary comparison](benchmarks/2026-10-16-room-churn-reuse-comparison.json)
and [routing guards](benchmarks/2026-10-16-room-churn-routing-guard.json) pass all
60 fresh processes. Separate [allocator diagnostics](benchmarks/2026-10-16-room-churn-memory.json)
measure about two fewer allocated objects and 27.2% fewer requested bytes per
complete membership cycle. This is the retained improvement; seven of eight
fanout ranges overlap, with fanout/private/reconnect median p99 buckets unchanged. Cache retention
and cold-start/allocator limits are recorded in the
[performance section](performance.md#recipient-views-during-membership-churn-2026-10-16).

The [1,024-reader check](benchmarks/2026-10-16-room-churn-reuse-upper-bound.json)
also runs an independent private pair and eight reconnect workers, verifies
396 complete in-room membership cycles and 809,984 broadcast deliveries,
then checks every controlled teardown departure and final cleanup. The same
selected binaries pass [C++ compatibility](benchmarks/2026-10-16-room-churn-reuse-reference.json),
[mixed-size socket stress](benchmarks/2026-10-16-room-churn-reuse-stability.json),
[descriptor pressure](benchmarks/2026-10-16-room-churn-reuse-accept.json),
[setup faults](benchmarks/2026-10-16-room-churn-reuse-setup.json),
[management API checks](benchmarks/2026-10-16-room-churn-reuse-api.json) and
[real Chrome acceptance](benchmarks/2026-10-16-room-churn-reuse-browser.json).
The small [two-worker CI command](benchmarks/2026-10-16-room-churn-ci.json)
passes locally with CPU affinity disabled; workflow `e52a953` runs it on Linux.
Hosted CI execution is not claimed. These finite local checks do not establish
continuous concurrent membership performance or production capacity.

## Bounded TCP admission and consistent listener queues, 2026-10-16

Runtime `8676e7e` coalesces pending listener continuations: one worker can retain
one accept token even if more readiness arrives before its next turn. The
initial actual-TCP regression fixture fails baseline with two tokens; the
selected fixture also verifies 128 refused sockets reaching EOF, exact counters
and physical admission retiring on worker teardown. Established socket work,
resource-error retries and shutdown retain their existing scheduling paths.

The initial data listener previously had backlog 128 while extra native workers
used 1,024. Both CLIs now expose `--listen-backlog` / `WAR_LISTEN_BACKLOG`, default
1,024 with positive `u16` bounds and OS clamping. Native embedding applies its
`Config` setting to the supplied listener; bridge embedding retains its supplied
listener's queue. Twelve [live setting checks](benchmarks/2026-10-16-accept-backlog-settings.json)
verify defaults, CLI/env overrides, every native listener and the host clamp.

The [48-process comparison](benchmarks/2026-10-16-accept-admission-comparison.json)
keeps one negotiated CBOR session alive while an independent process completes
raw policy-refused TCP lifecycles. It checks whole bodies, FIFO, EOF/reset,
all exact counters, zero routing copies and final resource retirement, in fresh
network namespaces. All ordinary cases pass. The
[1,024-client baseline diagnosis](benchmarks/2026-10-16-accept-flood-backlog-diagnosis.json)
retains a client deadline with 2,138 kernel listen drops while application work
continues. The selected [upper-bound check](benchmarks/2026-10-16-accept-backlog-upper-bound.json)
completes with zero kernel drops/retransmissions. Larger kernel queues consume
memory and worker time; the [performance record](performance.md#tcp-admission-continuations-and-backlogs-2026-10-16)
explicitly retains the saturated one-worker echo-throughput loss.

Six independent shutdown diagnostics cover before/coalesced/selected and one/four
workers with an active 256-client refusal flood. Each native process exits zero;
the held peer receives Close 1001 and EOF after eight verified echoes. The flood
client's expected exit one on listener closure is a cancellation result, separate
from completed normal workloads. Three [descriptor-pressure cases](benchmarks/2026-10-16-accept-backlog-accept.json)
and four [socket-setup fault cases](benchmarks/2026-10-16-accept-backlog-setup.json)
preserve recovery, held physical slots, queued admission and shutdown.

The same selected binaries pass [C++ compatibility](benchmarks/2026-10-16-accept-backlog-reference.json),
[four socket stress cases](benchmarks/2026-10-16-accept-backlog-stability.json),
[management API checks](benchmarks/2026-10-16-accept-backlog-api.json),
[real Chrome acceptance](benchmarks/2026-10-16-accept-backlog-browser.json) and
the small unpinned [CI command](benchmarks/2026-10-16-accept-backlog-ci.json).
The manifest retains 138 passing Rust tests and strict checks. Hosted CI and
deployment are unverified; these finite loopback cases are local acceptance.

## Established traffic during accept bursts, 2026-10-16

Runtime `d47f0fd` keeps the one-token listener continuation and reduces each
native accept turn to at most 16 sockets. Error retries, physical-slot ownership,
shutdown and shared body lifetimes retain their prior paths. The
[42-case admission comparison](benchmarks/2026-10-16-accept-fairness-comparison.json)
and [12 standard routing guards](benchmarks/2026-10-16-accept-fairness-routing-guard.json)
plus [six admitted-burst guards](benchmarks/2026-10-16-accept-fairness-burst-guard.json)
verify complete CBOR bodies/sender/FIFO, exact counters and final resource
retirement. The [performance section](performance.md#accept-turn-fairness-2026-10-16)
records the established-traffic benefit, modest refusal/reconnect throughput
costs, larger mixed-work CPU and unchanged ordinary-control p99 buckets.

Both native builds pass separate 1,024-client refusal cases without kernel
drops/retransmissions. Four shutdown cases cover before/selected, one/four
workers and an active 256-client flood. Each native process exits zero and the
held peer verifies eight echoes, Close 1001 and EOF; each flood client has the
expected cancellation exit one. Three [descriptor-pressure cases](benchmarks/2026-10-16-accept-fairness-accept.json)
and four [setup-fault cases](benchmarks/2026-10-16-accept-fairness-setup.json)
also preserve recovery and physical slots through TCP close.

The selected [1,024-reader changing-room check](benchmarks/2026-10-16-accept-fairness-fanout-upper.json)
verifies 347 complete membership cycles and 709,632 fanout deliveries, every
controlled departure and final retirement. Separate [socket stress](benchmarks/2026-10-16-accept-fairness-stability.json)
passes ordinary traffic, 4,096-source-IP reconnects, an unread peer and mixed-size
fanout. The same binaries pass [C++ compatibility](benchmarks/2026-10-16-accept-fairness-reference.json),
[management checks](benchmarks/2026-10-16-accept-fairness-api.json),
[real Chrome acceptance](benchmarks/2026-10-16-accept-fairness-browser.json),
138 Rust tests and strict checks. Two separate perf captures have zero lost
samples; no profile timings enter ordinary results. The small CI command passes
locally; hosted CI, deployment and production capacity remain unverified.

## Pending HTTP state and lifetime checks, 2026-10-16

Runtime `ceda58f` keeps resumable HTTP metadata inline, removing a state box
while preserving measured stage/connection sizes and input-buffer ownership.
The compaction guard now partially parses an HTTP request, moves its live slot,
finishes the negotiated upgrade and verifies a full CBOR body. The embedding
cancellation guard holds a pending HTTP descriptor beside 64 joined sockets,
then checks TCP closure, zero connections/read reservations and no handshake
error or disconnect fanout from cancellation.

The [48 pending-peer cases](benchmarks/2026-10-16-handshake-inline-comparison.json)
and 18 routing guards pass whole-body/FIFO, absolute five-second timeout,
no-response EOF/reset, physical admission, read-allowance reuse and final cleanup
checks. Four separate [allocator diagnostics](benchmarks/2026-10-16-handshake-inline-memory.json)
measure fewer requests and less live metadata; their timing is excluded from
ordinary results. The [performance record](performance.md#inline-http-upgrade-state-2026-10-16)
retains overlapping throughput ranges, moved p99 buckets and the measured limits.

The selected [8,192-idle-peer case](benchmarks/2026-10-16-handshake-inline-idle-upper.json)
and [1,024-client refusal case](benchmarks/2026-10-16-handshake-inline-admission-upper.json)
also complete. The [1,024-reader changing room](benchmarks/2026-10-16-handshake-inline-fanout-upper.json)
verifies 445 complete membership cycles, 910,336 fanout deliveries and every
controlled departure before cleanup. Descriptor exhaustion, accepted-socket
setup faults, active-flood shutdown, four mixed socket cases, C++ compatibility,
JavaScript, management and real Chrome checks pass. The small pending-header
CI command passes locally without CPU affinity. Hosted execution/deployment
and production capacity are outside these finite checks.

## Bounded service cleanup after task failure, 2026-10-18

Service teardown now keeps joining remaining tasks after an error or panic,
retaining the first failure within the existing outer shutdown deadline. The
previous early-return loop could drop the other join futures before native
worker retirement completed. Controlled regressions fail with the old loop and
pass with the selected drain helper; a never-finishing task still reaches the
outer deadline. This does not change routing, admission or packet ownership.

The [short shutdown evidence](benchmarks/2026-10-18-shutdown-drain/README.md)
records 89 working-tree library tests, strict Clippy, formatting, a release build
and a disposable two-worker SIGTERM check with four joined peers and an incomplete
upgrade. All joined peers receive `1001`, all transports reach EOF and the process
exits zero. Process shutdown flushes Close and retires TCP without waiting for a
peer acknowledgement; that existing policy is unchanged. Error/panic injection
targets the production task-drain helper, not a crashing native I/O thread.
