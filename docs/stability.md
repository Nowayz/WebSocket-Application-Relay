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
monotonic eight-byte counter per worker, verifies its self-message against the
new session ID, and completes a close handshake before reconnecting.

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
| 64 reconnect workers, up to 20 s | Failed after first wave: missing Close reply | 1,911,994 complete connection lifecycles |
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
