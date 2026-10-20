# Observer-specific departure barriers

Clearing an authenticated global listener's disconnect bit must put already
captured notices before its acknowledgment. The previous correct implementation
finished pending global-channel jobs, submitting output to every global recipient
while holding the registry write lock. One observer's control command could
therefore stall unrelated users for hundreds of milliseconds.

The revised barrier submits only that observer's still-unvisited notices, in the
shared departure FIFO's order. A monotonically increasing publication sequence
and a per-peer watermark prevent later worker visits from replaying those notices,
even after re-subscription. Already-visited recipients are not sent duplicates.
Ordinary rooms and other observers retain their normal 1,024-recipient turns.
The acknowledgment also uses the ready-output barrier path: a full writable
one-slot queue can drain before reserving the next message slot.

Publication still takes the registry lock and then the existing retirement mutex;
advancement takes only the retirement mutex. The barrier holds both in that same
order, so no departure can publish across its captured boundary. Watermark access
is serialized by the retirement mutex. The counter survives an emptied queue;
exhaustion uses ordered synchronous fallback rather than wrapping. No additional
lock, recipient index, payload copy or per-barrier allocation is introduced.
Published shared bodies and recipient views keep their existing charges until
all remaining work finishes. Application-message fanout does not read watermarks.

A compiler-layout census on the measured host found `Peer` grew from 80 to 88
bytes; `Budget` stayed 32 bytes and `RoomFanout` stayed 112 bytes. The explicit
watermark cost is eight bytes per peer, about 0.76 MiB for 100,000 peers, plus one
counter per relay. This is a measured layout cost, not an exact RSS prediction:
allocator granularity and other retained storage also affect process memory.

## Short 100,000-observer event comparison

Four serial control/candidate/candidate/control runs used eight pinned server
cores and separate client cores on the same host. Each process held 100,000
audience observers plus the tested observer, a departure source and one independent
echo witness. The fixture checked the old notice before the unsubscribe reply,
no late duplicate, a fresh notice after re-subscription, all audience departures,
zero residual queued credit, zero recorded routing-copy bytes and 100,000 shutdown
Close `1001` receipts. Every server exited normally without a forced kill.

| Run | Unsubscribe acknowledgment, ms | Worst witness RTT overlapping the command, ms |
| --- | ---: | ---: |
| Control 1 | 348.204 | 347.962 |
| Candidate 2 | 0.971 | 0.663 |
| Candidate 3 | 0.961 | 0.669 |
| Control 4 | 343.179 | 343.011 |

The median acknowledgment fell from 345.692 to 0.966 ms, about 99.72% lower for
this event. Every row included an overlapping witness sample. The witness's
maximum across its entire short sampling period was 1.247–2.486 ms for the
candidates. These are loopback command/event measurements including client and
kernel scheduling, not server-only timings, endurance results or every user's
latency during the event.

The original one-slot fixture passed revocation/re-subscription but then failed
on both control and candidate: it published two later departures before draining
their recipients. That required absorbing a burst larger than the configured
one-message queue. Those complete cases remain failed evidence, excluded from the
table. The maintained fixture drains each follow-up departure before publishing
the next. Its 100,000-observer candidate check with `--queue-messages 1` passed
at 0.926 ms and verified all 100,000 shutdown Close receipts. Optional
`--burst-teardown` retains the separate queue-pressure diagnostic.
A repeat with diagnostic frame details confirmed a recipient Close `1013`,
`Try Again Later`, during that burst. It remains a failed pressure case, not
evidence that a one-slot queue can absorb arbitrary concurrent departures.

## Ordinary mixed-load regression screen

Four more serial runs used 100,000 capacity users, 8,192 room multicast readers,
32 private pairs, two inspectors, eight churn workers at 32 cycles/s and eight
offered four-message fanout windows/s. The steady phase was two seconds; setup,
proof delivery and exhaustive cleanup take additional wall time. Each capacity
user supplied eight observations, so its reported p99 equals its observed maximum.

| Run | Server CPU, s | Private deliveries/s | Median user's observed maximum, ms | Worst user, ms | Worst fanout window, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.37 | 1,335,673 | 0.928 | 3.856 | 64.301 |
| Candidate 2 | 9.31 | 1,058,713 | 1.146 | 4.782 | 75.108 |
| Candidate 3 | 9.36 | 1,288,932 | 1.029 | 4.673 | 62.008 |
| Control 4 | 8.95 | 950,144 | 1.185 | 11.754 | 78.867 |

Every run completed all 16 offered windows, 524,288 multicast deliveries, 512
churn cycles and 800,000 checked capacity-user observations, along with the
capacity departure matrix and final resource checks. All process exit codes were
`[0,0,0]`. Intended-send worst latency, which includes generator scheduling,
ranged from 5.950–12.816 ms for controls and 6.815–8.436 ms for candidates.

The ranges overlap. Candidate 2 had lower private throughput and higher sampled
latency than Control 1; the reverse-order repeats did not establish a consistent
ordinary-load regression or gain. These four short screens do not certify a
long-term per-user p99. Selection is justified by the repeatable targeted barrier
benefit with checked semantics, not a claimed general throughput improvement.

## Limits and further work

The initial observer-specific barrier still searched each queued global view.
The [shared-view follow-up](#shared-view-lookup-follow-up) below removes repeated
searches of the same view. Distinct generations still need linear searches under
the registry lock, and retirement advancement still serializes output turns.
Queue-budget refusal uses [fallback scoped to overlapping channels](retirement-fallback.md);
slow-recipient and exhausted-credit policies are unchanged. Keep enough metadata
headroom for captured views.

Further optimization should establish the remaining source of long control turns
or close bursts with a short profile, then include normal mixed traffic and
all-user intended-send measurements before changing membership or scheduling.

## Reproduction and source identity

The control is consolidation commit `f58441c`. Frozen release SHA-256 identities:

- Control server: `ca98c5e5fce778706ca7f731f803c7153286e29809983299777da090fa9f41aa`.
- Selected candidate server: `1668b00176a4b9d5cc1610128108c8e335636f79471afb7b8497d5d0b7b39738`.

The first four event rows used the same frozen fixture, SHA-256
`fbcf371f14cb5573bf79af34e24b801c30eaca46dbce00282854d208705bf336`.
The maintained `scripts/observer-barrier.py` derives from that fixture, adds
portable affinity/worker options and helper identities, improves failure output,
and separates sequential from burst teardown. That revision supplies the
one-slot and small CI checks. It uses independent encoded native CBOR, without
the JavaScript library or an alternate wire protocol.

On the measured 32-logical-CPU host, repeat with fresh output names:

```sh
taskset -c 8-15,24-31 python3 scripts/observer-barrier.py \
  --server /absolute/path/to/frozen/war-relay --server-cpus 0-7 \
  --audience 100000 --output artifacts/observer-event.json

python3 scripts/mixed-capacity.py \
  --server /absolute/path/to/frozen/war-relay \
  --capacity-client /absolute/path/to/frozen/capacity \
  --sidecar /absolute/path/to/frozen/inspection \
  --users 100000 --seconds 2 --readers 8192 --scope room --pairs 32 \
  --churn-workers 8 --churn-cycles-per-second 32 \
  --fanout-windows-per-second 8 --pin-client-workers \
  --output artifacts/observer-mixed.json
```

Adjust masks for another host. Use identical frozen clients, serial builds/runs,
and no profiler for ordinary timing. Add `--queue-messages 1` to the event check
for bounded-message correctness. A portable small CI check uses `--audience 2048
--workers 2` and no CPU mask. The full Rust suite passed 208 tests; Clippy with
warnings denied, formatting, all 66 JavaScript tests, vendor provenance and the
maintained-product audit passed. The CI fixture also checks all 2,048 shutdown
Close receipts. No legacy or C/C++ implementation is reintroduced.

The [subsequent snapshot ownership change](retirement-snapshots.md) reuses and
charges overlapping disconnect views once. The footprint and comparison numbers
above describe this report's `28e97de` revision.


## Shared-view lookup follow-up

The later shared-snapshot design made repeated scans of the same view visible.
An observer revoking its disconnect flag still searched each queued job's
recipient vector independently. A 64-departure burst against 100,000 observers
searched essentially the same 100,001-member view repeatedly under the registry
and retirement locks.

A barrier now keeps one local borrowed view/position pair. Jobs sharing that
view reuse the result, including an absent result. The identity comparison uses
the slice's storage address and length, rather than its length or contents alone.
Every job separately checks whether its own cursor has already visited that
position. A changed view triggers a new lookup. The cache borrows from the locked
queue and disappears before the guard is released; it owns no recipient handles,
adds no allocation or per-peer field, and cannot survive into another command.
Watermarks, FIFO publication, permission checks, shared body ownership and queue
reservations retain their contracts.

This reduces repeated-view lookup work to one scan plus a descriptor check per
job. It leaves linear scans for distinct generations and can rescan a view if a
different global view intervenes. The initial lookup searches the whole view to
obtain a reusable position; an almost-finished single job can inspect an earlier
prefix that the previous tail-only search skipped. This is a targeted optimization
for shared pending views, not a fixed-time membership index.

A disposable release component probe held 100,000 real registered listeners and
queued real departure jobs, with a synchronous non-retaining test output. Five
samples per size called the same barrier against an unchanged queue; the probe
restored only its test observer's watermark between samples. It isolates lookup
and submission cost, without transport, concurrent workers or publication timing.

| Pending jobs sharing the view | Control median, ms | Candidate median, ms |
| --- | ---: | ---: |
| 1 | 0.021570 | 0.020219 |
| 8 | 0.157976 | 0.020230 |
| 64 | 1.258325 | 0.021179 |
| 256 | 10.036373 | 0.024690 |

The 256-job row is component evidence with a non-retaining output, not a promise
that a native recipient can absorb a 256-message burst within its default queue.
Individual control samples varied, especially at 256 jobs. Production timing
uses separate uninstrumented frozen binaries.

The native control and final-candidate cases used the same
`scripts/observer-barrier.py`, eight workers on CPUs 0–7 and client cores
8–15,24–31, with 100,000 audience observers and 64 distinct departure sources.
Each checked 6,700,065 notices, their order, revocation/re-subscription, zero
residual queued credit, zero copy/error counters and 100,000 shutdown Close
`1001` receipts. All servers exited normally without a forced kill. Each complete
fixture took roughly half a minute, largely for population and exhaustive proof.

| Run | Revocation ACK, ms | Worst witness overlapping revocation, ms | Worst witness over short event, ms | First Close through ACK, ms |
| --- | ---: | ---: | ---: | ---: |
| Control 1 | 4.485 | 5.370 | 5.370 | 91.743 |
| Control 2 | 3.976 | 1.377 | 5.191 | 106.464 |
| Final candidate 1 | 1.010 | 2.970 | 4.540 | 94.310 |
| Final candidate 2 | 3.035 | 2.836 | 3.843 | 121.013 |

Both candidate ACKs were faster than both controls; the two-sample median fell
from 4.230 to 2.023 ms. Overlapping witness results were not uniformly better,
and whole close-burst ranges overlapped. Measurements include client/kernel
scheduling and do not establish a whole-burst gain, an all-user latency percentile
or a sustained SLO. The adopted benefit is faster revocation across shared pending
views with the complete wire/resource matrix intact.

The new regression covers equal-sized membership generations where an observer
is absent, then last, then first. Existing regressions verify shared jobs with
different progress cursors, mixed ordinary/global views, independent barriers,
watermark exhaustion and no late replay after re-subscription.

To reproduce the native burst, add `--departures 64` to the event command above.
The follow-up control is commit `0d16d35`, executable SHA-256
`35dae946d6de535b8edcd038411bcc33fc46000b8cdd1ad1d608c4f0db9a7cd6`.
The candidate executable SHA-256 is
`34bd5cc087e2d0bd87181d449b3d014baf2eeb9e0d8598f1a98cce5ca9769c90`.
Copied component/production sources, probe source, build receipts and native
receipts preserve the exact inputs. The maintained JavaScript client and native
CBOR protocol are unchanged.



Four two-second ordinary mixed-load screens also passed with 100,000 capacity
users, 8,192 room readers, 32 private pairs, two inspectors and paced churn/fanout.
Each completed all 16 offered windows, 524,288 multicast deliveries, 512 churn
cycles and 800,000 capacity observations, with exits `[0,0,0]`, final resources
zero and no copy/pressure/error violations.

| Run | Server CPU, seconds | Median user's observed maximum, ms | Worst observed user, ms | Private deliveries/s | Largest fanout window, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control, first | 8.93 | 0.949 | 4.334 | 1,162,949 | 71.634 |
| Earlier cache build | 9.25 | 1.116 | 4.842 | 1,037,696 | 74.250 |
| Final cache build | 9.31 | 1.118 | 8.149 | 1,192,299 | 73.039 |
| Control, repeat | 9.34 | 1.132 | 5.107 | 1,081,017 | 79.835 |

The earlier cache build used an equivalent negated guard; the native selection
rows and final verification refer to the final executable above. Its initial
ordinary pair had lower candidate private throughput. Reversing the order with
the final build produced better throughput and similar CPU/median latency than
the following control, but its worst sampled user remained slower. These short
screens have only eight observations per user; they establish neither a general
ordinary-forwarding gain nor tighter ordinary tails. The retained benefit is
faster shared-view revocation with unchanged correctness and memory contracts.

All 215 Rust tests and 66 JavaScript tests passed, along with formatting, Clippy
warnings denied, vendor provenance and the maintained Rust/CBOR product audit.
The JavaScript fixture retired all six resource gauges. Final native checks
also passed with 100,000 observers/eight departures under 2 MiB credit, verifying
1,100,009 notices, re-subscription, zero pressure/copy counters and 100,000 shutdown
Close receipts. The two-worker, 2,048-observer one-slot queue check passed too.

The subsequent [retirement scheduling change](retirement-turns.md) reduces native
departure turns to 128 recipient visits. The measurements above retain their
original 1,024-visit scheduling contract.
