# Bounded global broadcasts

Global broadcasts previously walked every addressable ID owner synchronously
while holding the registry read lock. Large publications could prevent a join
or removal from acquiring the write lock even when the joining user belonged
to an unrelated room. The old path also occupied its source worker until the
whole fanout was submitted.

The global route now captures an immutable owner view, drops the registry lock
and uses the existing room-fanout continuation. Native turns visit at most 1,024
recipients. Payload bodies remain shared, source processing preserves FIFO,
and snapshot storage is conservatively charged per publisher using the existing
queue budget. A stable population reuses its cached view. Joins, owner removals
and ID changes invalidate it; removing a displaced session cannot erase the
replacement owner's index. A captured publication excludes later joins and
keeps its own view alive. Cancellation retires the captured storage before
returning its credit. Shutdown discards the index before bulk retirement.

This adds one registry cache, with no new per-peer field or hot-path lock.
There is at most one current index bounded by admitted population; older views
retained by jobs are charged. The first capture after invalidation still takes
O(number of addressable owners) work under a read lock. Releasing an unshared
index also takes linear work. Channel/private observer message paths remain
synchronous because their eligibility and acknowledgment boundaries need a
separate continuation design.

## Short comparison on 2026-10-19

Four serialized release runs used control/candidate/candidate/control order,
the same frozen Rust client, eight pinned server workers on CPUs 0–7 and eight
client workers pinned within CPUs 8–15,24–31 on an AMD Ryzen AI MAX+ 395
(16 cores, 32 logical CPUs).
Each run retained 100,000 readers in distinct singleton rooms and sent two
1 KiB global publications at one window per second. After the first reader
acknowledged each window, a new independent peer joined, verified a self echo
and completed normal Close/EOF. It had to receive no earlier broadcast.
All readers verified both exact packets; setup and cleanup were gated outside
the approximately 2.17-second steady phase.

| Measure | Control, two runs | Candidate, two runs |
| --- | --- | --- |
| Worst join/echo/Close probe per run | 121.55 / 128.06 ms | 3.12 / 3.03 ms |
| Worst fanout receive sample per run | 148.59 / 157.37 ms | 150.78 / 147.45 ms |
| Steady-phase server CPU | 0.62 / 0.64 CPU-seconds | 0.60 / 0.59 CPU-seconds |
| Verified fanout deliveries per run | 200,000 | 200,000 |
| Measured RSS per run | 185.9 / 186.0 MiB | 186.1 / 183.3 MiB |

The measured improvement is independent membership responsiveness during a
global publication. Bulk delivery timing is similar in these short runs; the
small CPU difference does not establish a general efficiency improvement.
Fanout receive timing includes the Rust generator's notification/scheduling
work across 100,000 readers and transport/client processing. It is not an
isolated server-hop latency. There are only two observations per recipient and
two join probes per run; these are scheduling screens, not production p99s.

Every run passed exact wire/counter totals, all offered windows, unique reader
coverage, normal Close/EOF and final zero connection, session, queued, decoder,
output-arena and variable gauges. Routing reported 2,056 reused value bytes and
zero copied value bytes per run. Warm allocator RSS remains after logical
cleanup and is not itself evidence of a leak.

The ordinary-room mixed screen was also run in control/candidate/candidate/control
order. Each run held 100,000 private-probe sessions, added 8,192 room readers,
32 private pairs, inspection and paced churn, and completed all 16 offered
fanout windows. Each population user contributed eight private observations.
All four runs passed wire, ordering, offered-load, Close/EOF and resource gates.

| Mixed-screen measure | Control, two runs | Candidate, two runs |
| --- | --- | --- |
| Worst population private receive sample | 4.00 / 4.91 ms | 5.32 / 5.30 ms |
| Worst population intended-send sample | 6.76 / 6.85 ms | 6.77 / 8.67 ms |
| Steady-phase server CPU | 8.94 / 9.41 CPU-seconds | 9.29 / 9.35 CPU-seconds |

CPU ranges overlap, while the candidate's observed private tails were modestly
higher. These short screens do not prove a general speedup or identical private
latency. The change is retained for the large, repeated membership-latency
improvement during global broadcasts with similar bulk timing and bounded
storage; the mixed observations remain part of the evidence and follow-up work.

Control server SHA-256:
`b4fde7a2012be2279121a7a6e68491a14e0255343b69bbcc3da4c2f9355dcb5b`.
Candidate server SHA-256:
`538c6632140e8b9916de7e7756781d7c16e8702fe16e22ffe8542acfc6b6c6e5`.
Local source snapshots, build receipts, JSON, per-user CSV and failed fixture
development attempts are retained under
`/home/phil/dev/war_next/artifacts/global-fanout-20261019/`.
The two earlier fixture failures were a probe-inclusive fanout count and an
unpaged HTTP response exceeding the fixture's response buffer. Both happened
before accepted comparison runs; the final fixture checks a paginated census.

## Reproduce

Build the Rust server and `inspection` example, or use recorded frozen binaries:

```sh
python3 scripts/inspection.py --server /absolute/path/to/war-relay \
  --client /absolute/path/to/inspection \
  --rooms 0 --pairs 0 --mode none --inspectors 0 --churn-workers 0 \
  --fanout-readers 2048 --fanout-global-broadcast --fanout-window 2 \
  --fanout-windows-per-second 1 --fanout-latency-output artifacts/global-users.csv \
  --workers 2 --client-workers 2 --seconds 2 --server-cpus '' --client-cpus '' \
  --output artifacts/global-check.json
```

This CI-sized case checks multiple ordered packets per window. For the measured
case use 100,000 readers, window one, eight server/client workers, the masks above
and both pin flags. The fixture binds reader connections across loopback source
addresses to avoid the single-source ephemeral-port limit. Reader rooms are
isolated so teardown stays linear. Choose fresh output paths; never time builds
or competing benchmarks concurrently.
