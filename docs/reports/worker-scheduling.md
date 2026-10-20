# Bounded worker scheduling across publications

Bounding one publication does not bound a worker's complete ready batch. A
worker previously advanced up to 1,024 connections before polling again, and
each connection could advance a 1,024-recipient publication. Newly reported
readiness then joined the same FIFO behind buffered publication/output work.
The [concurrent-publisher screen](concurrent-observers.md) exposed response delays
that a faster observer-view lookup did not resolve.

## Scheduling and ownership

Each worker now owns two token FIFOs. Kernel readiness and due timers enter the
fresh queue; buffered continuations, accept continuations and output wakes enter
the existing queue. Each loop services both before polling again:

- Fresh readiness: at most 1,024 connection turns, with a 4,096-visit routing allowance.
- Buffered/output work: at most 1,024 connection turns, with its own 4,096-visit allowance.
- Deferred retirements retain their separate 128-visit turn.

Both queues receive service even when the other remains busy. A connection's
existing scheduled flag prevents duplicate entries across them. Readiness still
upgrades an already queued output-only wake, preserving input/deadline handling.
An unfinished connection rejoins the buffered queue; both nonempty queues force
a nonblocking poll, so edge-triggered buffered input cannot lose its continuation.

The source quantum and deferral cutoff remain 1,024 visits. A phase finishes the
current native turn before yielding, so its routing allowance can overshoot by
at most 1,023 charged visits. Continuation accounting conservatively charges a
full turn even when a job finishes early. Successful new routing reports its
work count. This avoids an extra job lock to count the continuation remainder.
An initial routing failure currently returns before that count is charged;
those attempts still have the connection-turn bound. Conservative failure
charging remains a follow-up. Control-handler scans, cold-view construction,
output-byte work and teardown have their own costs; this is not a wall-clock
execution bound.

An initial prototype shrank each source's quantum to the remaining phase credit.
A regression caught that this could make a small fanout allocate staging merely
because earlier connections had spent the worker allowance. That prototype is
rejected. The retained policy yields after the ordinary source turn and preserves
small-fanout classification and staging requirements.

The queues contain stable tokens, never payloads or copied recipient lists.
Their backing storage grows to worker high-water demand, bounded by admitted
connections; a connection occupies one scheduler queue at a time. The second
deque adds warm scheduler capacity, but no peer field, global lock, per-message
scheduler allocation after capacity warmup or application-body copy. Existing
shared bodies, output serialization, source FIFO, barrier watermarks, queue
credit and cancellation ownership are unchanged.

## Profile and short native comparison on 2026-10-19

The control's user-cycle profile attributed 35.44% to `Peer::enqueue_fanout`,
13.02% to native `send_fanout` and 12.13% to `write_many`. It contained roughly
2,000 samples with no lost samples. It identifies the fanout/output path rather
than the observer lookup as the principal user-space cost in this screen; it
does not measure kernel-cycle proportions. Profiled timing is excluded below.

The AMD Ryzen AI MAX+ 395 host ran eight server workers on CPUs 0–7 and the
same frozen Python client on CPUs 8–15,24–31. Each run retained 100,000 observers
and used 64 singleton publishers alternating channel/private publications, two
rounds and eight-byte values. Every audience member's 128 expected publications
was checked: 12.8 million audience deliveries per run. Control/candidate/
candidate/control order used the `control-d`, `balanced-a`, `balanced-b`, `control-e`
receipts. Builds and measurements were serialized. Ranges contain four event
samples per version, not long-term percentiles:

| Measure | Control | Two queues, four-turn allowance |
| --- | --- | --- |
| Unsubscribe ACK | 6.08–12.70 ms | 2.43–4.63 ms |
| Independent join | 7.66–12.99 ms | 3.07–5.78 ms |
| Worst control RTT overlapping each run's publication events | 12.70–13.91 ms | 9.14–9.58 ms |
| Server CPU | 8.24–8.26 CPU-seconds | 8.50–8.51 CPU-seconds |
| Process RSS after audience drain | 318.1–338.3 MiB | 264.0–291.6 MiB |
| Retained output-queue capacity after drain | 184.8–215.1 MiB | 144.8–167.5 MiB |

All four runs passed exact wire/counter checks, zero copied routing bytes and
zero live queued bytes at the quiescent gate. Each verified all 100,000 retained
shutdown Close `1001` and transport EOF receipts. Retained queue capacity is
intentional warm storage, distinct from live queued bytes. These RSS/capacity
observations do not establish a general memory reduction.

Earlier output service trades batching for responsiveness. A one-turn allowance
had lower join/ACK times in some events but used 9.75–12.00 CPU-seconds in the
two-queue burst screen. Four turns are retained as the better measured balance.
The native screen includes closed-loop witnesses and serial audience checking;
its CPU figures do not establish fixed-offered-rate efficiency or maximum bulk
throughput. No universal speedup or language ranking is inferred.

A 256-connection fresh-service cap improved the observer profile but failed the
ordinary-room offered-load gate: 15/16 windows completed, despite passing exact
delivery and cleanup checks. That variant is rejected. The final cap uses the
existing 1,024-connection limit for both queues while retaining independent
routing allowances. The failed receipt remains in the evidence; it is not counted
as an accepted performance run.

## Mixed population acceptance

Short mixed runs retain 100,000 private-probe users plus 8,192 fanout readers,
32 private pairs, inspection and paced churn. The steady phase is two seconds;
setup, CSV output and complete teardown take additional wall time. Each user has
eight checked private observations, 800,000 per run. All 16 offered fanout windows
must complete, and exact wire/counter, normal Close/EOF and final resource gates
must pass. Per-user maxima include every user; intended-send figures additionally
include generator delay. The raw receipts and per-user comparison figure record
the final measurements and sample scope.

The observer-load comparison used `mixed-control-b`, `mixed-balanced-a`,
`mixed-balanced-b`, `mixed-control-c` in control/candidate/candidate/control order.
Ranges below cover two runs per version. Percentiles rank each user's maximum
of eight observations; they are not pooled packet percentiles:

| Mixed observer-load measure | Control | Selected scheduler |
| --- | --- | --- |
| Users' worst receive sample: 95th percentile | 3.41–7.72 ms | 1.76–2.45 ms |
| Users' worst receive sample: 99th percentile | 3.89–11.06 ms | 2.43–3.16 ms |
| Worst receive sample across all users | 4.92–28.84 ms | 3.53–4.89 ms |
| Worst intended-send sample across all users | 6.71–43.04 ms | 6.56–10.01 ms |
| Steady server CPU | 9.32–9.38 CPU-seconds | 9.23–9.38 CPU-seconds |

All four runs completed 16/16 offered windows and the full 800,000-observation
private matrix. The control's larger tail event and the candidate's higher
intended-send maximum in one repeat are retained in the report and CDF; these
measurements include generator/kernel scheduling and do not show every packet
becoming faster. Earlier exploratory and rejected-variant runs remain separate
from this selected comparison.

The change is retained for lower concurrent-publication response delays while
meeting the mixed population's offered work and cleanup contract. The burst CPU
tradeoff remains explicit. Eight observations per user cannot establish a
production p99 or latency deadline.

The ordinary-room guard used `room-control`, `room-balanced-a`,
`room-balanced-b`, `room-control-b`, again completing all 16 offered windows in
every selected run. The 95th percentile of users' worst receive samples was
2.78–3.55 ms for controls and 1.66–2.33 ms for candidates; the corresponding 99th
percentiles were 3.61–4.26 ms and 2.40–2.88 ms. Worst receive samples were
4.56–5.36 ms versus 3.92–5.66 ms, and worst intended-send samples were
6.61–7.37 ms versus 5.75–9.29 ms. Server CPU was 9.29–9.33 versus 9.38 CPU-seconds.
This supports better user-level distributions with similar CPU, while retaining
the slightly worse candidate maximum in one repeat. It does not show every
ordinary-room tail event improving.

## Verification and remaining work

Deterministic checks cover a partly spent worker allowance across room, global,
channel-observer and private-observer publications; exact FIFO; no read-ahead;
continued payload charging; and no additional staging for small fanouts. Arena
compaction preserves both token queues. Existing input/output reason-upgrade,
expiry, shutdown, source cancellation and native socket ownership checks remain
part of acceptance.

All 236 Rust tests and 66 JavaScript tests passed; the manual release diagnostic
remains ignored by ordinary tests. Formatting and Clippy with warnings denied
passed. The selected executable also passed the four native observer-message
cases, the 100,000-observer/eight-departure check with 2 MiB shared queue credit
(1,100,009 checked notices and 100,000 shutdown Close/EOF receipts), overlap
fallback, primed shutdown, the independent malformed/fragmented/pipelined input
corpus and one/four-worker descriptor-pressure recovery. Product and vendor
audits checked 219 maintained files and 62 pinned vendored files separately.

`fairness-observers` and `fairness-room` PNG/PDF/JSON artifacts compare both
repeats for every user. Their summaries retain source CSV digests and the eight-
observation scope. The room chart retains the slightly worse candidate maximum
rather than pooling users or excluding it.

The [routing refusal follow-up](rejection-accounting.md) now charges failed
initial attempts for their visited handles, preserving cleanup progress under
rejection pressure. Cold/changing recipient views and observer barriers under
churn remain future performance work; release readiness takes priority.
The new scheduler does not remove their membership-lock scans. Already queued
output work is upgraded in place when input becomes ready, rather than moved to
the fresh queue; its buffered turn remains bounded. Slow-consumer write service,
control-handler work and complete per-recipient fanout timing remain separate
measurement targets as supported traffic profiles grow.

Control server SHA-256:
`5f27499f27159a3bbaa9f5de5311bb1b6646719e681723503313e69eed4053c9`.
Selected server SHA-256:
`a7b2350b129c7882995e3d3dcd3dba14d476f7c7adbdc53f173c733568eabd2a`.
Frozen Cargo inputs, executable/build receipts, clients/helpers, profile, rejected
prototype evidence, raw JSON/CSV, figures and final selection are local under
`/home/phil/dev/war_next/artifacts/worker-budget-20261019/`.
