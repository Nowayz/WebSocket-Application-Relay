# Bounded observer publications

The [observer-message baseline](observer-messages.md) reproduced large stalls
while a publisher submitted an entire observer audience under the membership
read lock. Native channel/private observer publications now capture eligibility,
register their pending work and advance outside that lock in 1,024-visit turns.
Small fanouts retain their synchronous path. Unobserved private sends retain
their immediate borrowed-write path without entering the publication index.

## Ownership and ordering

Each publishing connection reuses its continuation box and observer job. One
job holds a charged shared body, an immutable ordinary-room view when needed,
an immutable observer view and a cursor across both segments. This avoids both
copying a combined recipient list and reserving the same source body twice.
Primary private submission occurs before observer capture. A subsequent staging
refusal cannot undo already submitted output; ordinary overload and slow-peer
policy still apply.

Registration holds the membership read guard until the job enters an ordered
index. Revocation holds the membership write guard, walks older indexed jobs
and submits only the requesting observer's unvisited messages before its
acknowledgment. A watermark is updated while each job is locked, preventing a
concurrent source from replaying that message. Ascending publication sequence
prevents a watermark from suppressing another unflushed job. Message classes
share a watermark; disconnect notices keep their existing separate barrier.

Workers lock only their own publication during a turn. The index is locked for
registration, unregistering and revocation, with the job lock released before
unregistering. There is no global publication-progress mutex. A weak view identity
reuses observer lookups across adjacent jobs sharing a view without retaining
its handle buffer after the owning jobs complete. Source input remains behind
its pending publication, preserving per-connection FIFO. Concurrent sources
have no global total-order guarantee.

## Storage and limits

There is one additional 64-bit watermark per peer, plus reusable job metadata
for connections that publish large observer fanouts. The ordered index can
allocate nodes during registration. At most two current observer routing views
are cached, bounded by admitted membership; flag changes invalidate the relevant
class. Pending jobs conservatively charge each captured view against the global
queue budget, as ordinary room jobs do. One body reservation survives across both
segments and is returned after completion or cancellation. Terminal shutdown
discards the current observer views before bulk retirement.

Cold view construction and invalidation remain linear in membership. Revocation
still scans distinct or alternating captured views under the membership write lock.
Many active publishers and frequent membership changes therefore remain a
focused measurement target. The [concurrent-publisher follow-up](concurrent-observers.md)
adds the native mixed-class fixture and records a rejected scan-cache experiment.
This change does not promise zero allocation,
zero kernel copying, lossless fanout under exhausted budgets or an unconditional
100,000-user latency guarantee.

## Validation

Deterministic tests cover partial traversal, source FIFO, pointer-identical
payloads, primary deduplication, a global source observing its own private send,
cross-class and maximum-sequence barriers, immediate re-subscription, membership
invalidation, cancellation, budget refusal and ID takeover. A gated submission
proves the membership lock is free while revocation waits for one job turn.
A combined room/observer test funds a roughly 900-byte value with a 1,034-byte
local queue limit and verifies a single body charge and independent room progress.

The native combined fixture verifies 2,048 room recipients plus 2,048 observers
with one output slot and a 2 KiB local byte budget. It checks source departure
notifications and all 4,096 retained shutdown Close/EOF receipts. Separate
channel/private cases and the short 100,000-user mixed runner retain exact
wire/counter totals, offered-load gates and cleanup checks.

## Short control/candidate comparison on 2026-10-19

For each message class, serialized release runs used control/candidate/candidate/
control order and the same frozen Python fixture. Each run retained 100,000
observers plus ten source/target/witness sessions and published two 1 KiB values.
The AMD Ryzen AI MAX+ 395 host had eight server workers pinned to CPUs 0–7;
the generator used CPUs 8–15,24–31. The following ranges cover four events per
class/version. They are finite observations, not long-term percentiles:

| Measure | Control | Bounded publications |
| --- | --- | --- |
| Channel-observer unsubscribe ACK | 20.34–103.15 ms | 0.55–0.92 ms |
| Channel-observer independent join | 42.01–46.59 ms | 0.75–1.30 ms |
| Channel-event worst overlapping private-echo RTT | 93.48–103.02 ms | 1.10–7.37 ms |
| Private-observer unsubscribe ACK | 26.79–49.08 ms | 0.83–1.06 ms |
| Private-observer independent join | 27.51–49.05 ms | 1.03–2.71 ms |
| Private-event worst overlapping control RTT | 33.63–118.54 ms | 1.74–8.56 ms |

Every publication and retained shutdown Close/EOF passed. Routing copied-value
bytes and live queued bytes at the quiescent gate were zero. Server CPU for the
two-publication screens was 0.59–0.62 CPU-seconds for the control and approximately
0.59 for the candidate. These screens also contain closed-loop witnesses and
serial audience verification, so the CPU figures do not establish a general
efficiency improvement. Process RSS ranges overlapped; there is no claim of
reduced process memory or zero metadata overhead.

A separate combined case retained 50,000 ordinary-room readers and 50,000
observers with a 2 KiB local byte budget. Unsubscribe ACKs were 15.28/17.44 ms
on the control and 0.28/0.30 ms on the candidate; independent joins were
21.54/70.92 ms versus 0.37/1.15 ms. Both versions checked every publication,
ordinary-room source-departure notices and all 100,000 retained Close/EOF receipts.
Earlier two-body prototypes failed the deterministic tight-budget regression;
their successful one-segment timings are retained separately and are not the
selected implementation.

## Mixed 100,000-user check

Each two-second mixed run retained 100,000 private-probe users, added 8,192
fanout readers, 32 private pairs, inspection and paced churn. All runs completed
16/16 offered fanout windows, checked every user's eight private observations
(800,000 total), and passed exact wire/counter, normal Close/EOF and final
resource gates. Population setup, CSV output and complete teardown are additional
wall time. Measurements include the generator and kernel path.

| Mixed-screen measure | Control | Bounded publications |
| --- | --- | --- |
| Observer load: worst private receive sample | 20.60 ms | 5.28 ms |
| Observer load: worst intended-send sample | 22.21 ms | 6.63 ms |
| Observer load: steady server CPU | 9.42 CPU-seconds | 9.35 CPU-seconds |
| Ordinary-room load: worst private receive sample | 4.67 ms | 5.22 ms |
| Ordinary-room load: worst intended-send sample | 6.05 ms | 7.30 ms |
| Ordinary-room load: steady server CPU | 9.30 CPU-seconds | 9.36 CPU-seconds |

The change is retained for the large observer responsiveness improvement and
the mixed observer-tail improvement with similar CPU. The ordinary-room screen
was close overall but had slightly higher worst samples; it does not support a
claim that every workload became faster. Per-user CDFs and their source CSV
digests are retained with the raw evidence. Eight observations per user cannot
establish a production p99 or a latency deadline.
In the observer-load sample, the fraction of users whose eight measured receive
times were all at or below 5 ms rose from 72.463% to 99.971%. Including intended
send and generator delay, the corresponding fractions were 67.428% and 98.974%.
These are descriptive threshold counts for this finite screen, not new deadlines.

All 233 Rust tests, 66 JavaScript tests and six fixture guard tests passed.
The 100,000-observer/eight-departure pressure check passed 1,100,009 notice
checks and all 100,000 retained shutdown receipts with 2 MiB shared queue credit.
Native overlap fallback, primed shutdown and the independent malformed/fragmented/
pipelined CBOR input corpus also passed. Vendor and maintained-product audits
remain separate checks.

Control server SHA-256:
`538c6632140e8b9916de7e7756781d7c16e8702fe16e22ffe8542acfc6b6c6e5`.
Selected server SHA-256:
`5f27499f27159a3bbaa9f5de5311bb1b6646719e681723503313e69eed4053c9`.
Frozen Cargo inputs, client/helper sources, builds, negative regressions, JSON,
CSV and figures are local under
`/home/phil/dev/war_next/artifacts/observer-publications-20261019/`.
The selected comparison uses the `composite` receipts. Initial and intermediate
prototype receipts remain available but are excluded from these final tables.
