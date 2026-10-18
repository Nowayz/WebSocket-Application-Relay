# Management polling competes with real-time traffic

This short diagnostic checks registry-size scaling before adding an ordered
index or shared snapshot cache. No server implementation or default changed.
The tested native executable is the empty-page candidate from the
[previous experiment](../2026-10-18-empty-channel-page/README.md), now adopted
in source at `79195d7`. Each JSON records its executable SHA and exact command.
The running selected services were not restarted.

## Workload and evidence

Every ordinary run has a two-second measured phase, with setup, verified
pagination and cleanup outside that phase. Four native workers use CPUs 0–3;
eight individually pinned client workers use 4–7,20–23. There are 64 private
pairs exchanging verified 1 KiB bodies in windows of 16. HTTP mode adds two
inspectors reading first pages of 1,000 channels every 10 ms (200 queries/s
combined). `none` mode disables those inspectors. Four churn workers, when
enabled, join, verify self-delivery and complete normal departure repeatedly.
Each run checks exact traffic, FIFO, listing metadata and resource retirement.

All 13 ordinary runs and the separate profile passed. None overlapped builds,
profiling or another benchmark. These are finite loopback screens without TLS,
not long-term per-user fairness measurements. Histogram p99 values below are
bucket upper bounds, not exact quantiles or confidence intervals.

| Rooms; churn workers | Inspection | Private million messages/s | Private p99 upper ms | Churn lifecycles/s |
| --- | --- | ---: | ---: | ---: |
| 1,024; 0 | None | 2.351 | 1.049 | — |
| 1,024; 0 | 10 ms | 2.353 | 1.049 | — |
| 8,192; 0 | None, two runs | 2.257–2.370 | 1.049 | — |
| 8,192; 0 | 10 ms, two runs | 2.224–2.265 | 2.097 | — |
| 1,024; 4 | None | 2.324 | 1.049 | 2,506 |
| 1,024; 4 | 10 ms | 2.298 | 1.049 | 2,497 |
| 8,192; 4 | None | 2.325 | 1.049 | 2,511 |
| 8,192; 4 | 10 ms | 2.133 | 2.097 | 2,265 |

At 8,192 rooms with churn, the HTTP run completed approximately 8.2% fewer
private messages and 9.8% fewer lifecycles than the no-inspection run. This
identifies management polling as material work in this fixture; it does not
attribute the entire difference to lock contention. Registry scanning, heap
selection, serialization, HTTP and client validation all contribute.

A separate two-second `cycles:u` profile with the larger registry and churn had
zero lost samples. Self samples attributed 5.57% to `channel_page`, 2.37% to
`BinaryHeap::sift_down_range`, and 0.75% to base64 encoding. These are sampled
symbols, not exact request costs or total inclusive cost. Raw perf data remains
local at `artifacts/page-scaling-profile.perf.data`; the text report is archived.
Profile-run throughput is excluded from ordinary comparisons.

## Cadence experiment before adding complexity

The larger registry/churn case then ran at 100 ms, 10 ms, 100 ms, respectively:

| Poll interval | Verified HTTP queries | Private million messages/s | Private p99 upper ms | Churn lifecycles/s | Churn p99 upper ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| 100 ms | 40 | 2.218 | 2.097 | 2,334 | 8.389 |
| 10 ms | 400 | 2.084 | 2.097 | 2,232 | 4.194 |
| 100 ms | 40 | 2.284 | 1.049 | 2,464 | 4.194 |

Less frequent polling recovered throughput in both surrounding runs but did
not consistently improve every latency measure: the first 100 ms run had a
worse churn p99 bucket. No universal latency benefit is established. CPU
seconds overlap as useful throughput changes; this is not a CPU savings claim.

For an operator-facing channel browser, consider starting at 100 ms or slower
and measuring the freshness requirement against routing/churn latency. This
is application guidance, not a new API limit, default or guarantee. Avoid
repeated complete traversals when only one visible page is needed. The API
continues to permit frequent inspection and provides fresh coherent pages.

An ordered registry index might reduce scans, but it would add mutation cost,
memory and synchronization to joins/departures. A cache would also need an
explicit freshness and memory contract. This screen does not justify either
change yet. The next architectural experiment should measure registry lock
hold/wait times separately and include churn before selecting such a design.

## Reproduction and limitations

Each raw JSON includes the exact server/client commands, affinity checks,
phase counters, fixture source hashes and binary identities. Use
`scripts/inspection.py` with `--seconds 2 --pairs 64 --inspectors 2`,
`--rooms 1024` or `8192`, `--churn-workers 0` or `4`, and `--mode none` or
`http`; only HTTP mode accepts `--page-size 1000`. Cadence changes use
`--inspection-interval-ms 100`. All other settings are recorded verbatim.
No source change or new public benchmark interface was needed.

Two failed fixture launches are retained: `--page-size` with mode `none` is
invalid, and 32,768 rooms exceeds the client's 8,192-room bound. Both stopped
before the measured workload and are excluded. The server also defaults to
10,000 channels/connections. This report makes no 32,768- or 100,000-channel
claim and does not weaken those safeguards to make a test pass.

`summary.json` extracts observations; `SHA256SUMS` covers archived evidence.
The three pre-existing user-modified files were verified byte-identical after
all runs. The selected native, bridge and static-server processes remain live.
