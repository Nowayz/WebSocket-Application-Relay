# Discard terminal room indexes before bulk retirement

A room keeps an immutable recipient vector for repeated broadcasts. If no sender
still owns a captured view, normal membership changes update that vector in place.
Removal searches for the departing key and then swaps out its entry. That saves
allocations during ordinary churn, but repeatedly searching a large warmed vector
while retiring every member makes terminal teardown quadratic.

Terminal removal now discards the room's cached vector before changing membership.
It has no future routing index to maintain. A captured fanout retains its own
immutable owner until it completes or is cancelled. Reversible drain still uses
the normal maintenance path, since existing sessions can keep communicating and
admission can resume. No payload copy, allocation, lock, per-peer field or deadline
change is introduced by this terminal branch.

## Release component screen

A disposable release-test snapshot populated one ordinary room, optionally routed
a real CBOR broadcast through the relay, confirmed a unique cached view, then
stopped the relay and dropped registrations. This isolates registry teardown;
it is not a network benchmark. The same probe ran before and after.

| Members | Uncached control, ms | Cached control, ms | Cached candidate, ms |
| --- | ---: | ---: | ---: |
| 4,000 | 0.595 | 1.843 | 0.483 |
| 8,000 | 1.028 | 5.174 | 1.014 |
| 16,000 | 2.174 | 21.391 | 2.164 |

The cached control's time grows much faster than the population. The candidate
restores approximately linear behavior in this screen. Absolute component times
include allocator and host scheduling effects.

## Checked 100,000-connection shutdown

Each disposable process admits 100,000 sessions to one room. Its first member
broadcasts a fixed CBOR byte string; the fixture verifies all 99,999 recipients.
A subsequent self-echo proves that publisher completed its bounded continuation.
The fixture then requires 100,000 physical/logical sessions, zero queued credit
and zero routing-copy/error counters before sending SIGTERM.

Every connection must receive Close `1001` followed by TCP EOF. All rows verified
all 100,000 Close frames and EOFs, exited normally and needed no forced kill.
Server workers used CPUs 0–7; the client used 8–15,24–31. Runs and builds were
serialized, with the ten-second default grace unchanged.

| Serial run | Signal through complete client drain/process wait, ms | First client's TCP EOF, ms |
| --- | ---: | ---: |
| Control 1 | 2,836.784 | 1,986.030 |
| Candidate 2 | 878.831 | 80.503 |
| Candidate 3 | 891.527 | 79.729 |
| Control 4 | 2,830.982 | 1,993.208 |

Median end-to-end cleanup fell from 2,833.883 to 885.179 ms, about 68.8% lower.
The first EOF moved from roughly 1.99 seconds to 80 ms. These intervals include
client reads and socket cleanup, not just server execution. A sequential drain
does not measure each client's independent arrival latency.

Without a priming broadcast, the control/candidate check completed in 880.842 /
871.755 ms. This supports the diagnosis that maintaining the warmed index caused
the added cost; it does not establish a general unprimed shutdown improvement.

The existing terminal policy flushes Close and retires transport without awaiting
peer acknowledgments. An initial small fixture injected ACKs and observed a reset;
that diagnostic is retained as a failed stronger-handshake check. The accepted
fixture checks the existing Close-plus-EOF contract and does not inject ACKs.
This change does not alter server Close policy or shorten grace.

## Normal behavior and verification

All 213 Rust tests and 66 JavaScript tests passed, along with formatting, Clippy
warnings denied, vendor provenance and the product audit. New regressions verify
unique and externally captured views during terminal removal, retained ownership
until a captured view is dropped, and live-cache consistency across drain/resume.
The small native CI fixture verifies a primed 2,048-session room and every EOF.

A two-second mixed-load control/candidate pair also passed with 100,000 capacity
users, 8,192 room readers, 32 private pairs, two inspectors, paced churn and fanout.
Both completed all 16 offered windows, 524,288 multicast deliveries, 512 churn
cycles and 800,000 capacity observations, exited `[0,0,0]` and retired resources.
Server CPU was 9.29 / 9.28 seconds. Median users' observed maxima were 1.099 /
1.225 ms; worst observed users were 4.775 / 5.299 ms. Private throughput was
1,204,131 / 1,062,827 deliveries/s, and maximum fanout windows 69.639 / 73.809 ms.
This single short pair checks normal correctness and resource behavior. Its
throughput/latency variation is not evidence of a normal forwarding improvement
or a sustained per-user SLO; each user supplied only eight observations.

## Reproduction and limits

```sh
taskset -c 8-15,24-31 python3 scripts/shutdown-capacity.py \
  --server /absolute/path/to/frozen/war-relay --server-cpus 0-7 \
  --users 100000 --prime-broadcast --output artifacts/shutdown-warm.json
```

Omit `--prime-broadcast` for the uncached comparison. Adjust masks for another
host. A small CI check uses `--users 2048 --workers 2` without masks. Output files
refuse reuse; receipts include commands, executable/fixture/helper digests,
priming proof counts, resource checks, per-connection Close/EOF counts and errors.

The control is Rust commit `db6755e`, executable SHA-256
`c87aed67d7b4e2c077c6fa025f2b1f8bcbd799edcf118fcadbdfcd13473c5c44`.
The selected candidate executable is
`002aaaeee203a9e29d3231eef1a09dae65da43df53704decae486ed9908f5bdd`.
The component probe is preserved with its source snapshots and logs; production
measurements use uninstrumented frozen executables.

Normal membership changes retain their existing cache behavior. First capture,
final view release, pending-view scans and synchronous queue-pressure fallback
remain separate profiling targets. This optimization addresses obsolete terminal
index maintenance; it does not complete the overall 100,000-user reliability goal.
