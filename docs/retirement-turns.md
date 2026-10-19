# Shorter native retirement turns

Deferred departures share a FIFO whose mutex also serializes publication and
observer barriers. Native workers previously advanced 1,024 recipients before
returning to socket readiness and polling. This preserved ordering and throughput,
but repeated source closes could wait behind those turns.

## Diagnose before changing the scheduler

A disposable diagnostic build measured the same 100,000-observer, 64-source
native burst as the production comparison. Across 64 Close acknowledgments:

| Diagnostic component | Total, ms |
| --- | ---: |
| Client-observed Close round trips | 97.234 |
| Registry write-lock acquisition | 0.003 |
| Retirement mutex acquisition | 15.679 |
| Publication after acquiring that mutex | 0.059 |
| Other membership-removal work | 1.314 |
| Time outside membership removal | 80.179 |

This identified real retirement contention, rather than expensive job allocation
or registry acquisition. The residual includes worker scheduling, transport and
client scheduling; the instrumentation does not isolate those components.
Diagnostic timestamps/logging are confined to a copied source snapshot. That
build supplies diagnosis, never a production timing row.

## Scheduling and ownership

The native worker now supplies a **128-recipient retirement budget**. Ordinary
live fanout retains 1,024-recipient turns. Separating those budgets shortens both
the shared publication critical section and the worker's background turn without
changing the live message batching policy. The relay accepts an explicit visit
budget; tests can exercise different budgets without baking the native scheduler's
policy into routing semantics.

Jobs stay in the same FIFO, with the same captured recipients, shared payloads,
source reservations, observer watermarks and global accounting. There is no new
lock, queue, per-peer field, allocation policy or configuration flag. Small-room
synchronous delivery, the 1,024-recipient deferral threshold, deadlines and
backpressure policy retain their behavior. A recipient-count bound is not an
elapsed-time deadline under OS preemption or memory stalls.

## Complete native burst comparison

Every uninstrumented row checked 6,700,065 notices in order, re-subscription,
zero residual queued credit, zero recorded routing-copy/error counters and
100,000 shutdown Close `1001` receipts. Processes exited normally without a
forced kill. Eight server workers used CPUs 0–7; clients used 8–15,24–31.

| Build / turn size | First Close through revoke ACK, ms | Median Close RTT, ms | Worst Close RTT, ms | Worst short-event witness RTT, ms | Completed-work server CPU, s |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 / 1,024 | 115.495 | 1.758 | 4.271 | 6.298 | 18.54 |
| Experimental / 256 | 31.812 | 0.439 | 1.600 | 0.809 | 19.15 |
| Experimental / 128 | 14.992 | 0.197 | 0.736 | 0.599 | 19.92 |
| Control 2 / 1,024 | 93.438 | 1.239 | 3.096 | 4.446 | 18.69 |
| Selected 1 / 128 | 16.309 | 0.223 | 0.813 | 0.553 | 19.92 |
| Selected 2 / 128 | 16.767 | 0.200 | 0.810 | 0.994 | 19.87 |
| Control 3 / 1,024 | 140.242 | 1.780 | 10.362 | 5.326 | 18.84 |

The 128-visit policy is selected for responsiveness, with a measured CPU cost:
about 5–7% more process CPU over the completed fixed notification matrix. CPU
excludes population setup and includes verification/follow-up traffic through
the final quiescent snapshot. It is not CPU during just the short Close window.
Witness sample counts differ with event duration, so their maxima are finite
event diagnostics rather than comparable long-term tail estimates.

The final fixture also records when the first audience reader finishes all 64
original notices. Since later FIFO jobs cannot pass earlier jobs' traversal,
this waits for all preceding jobs' submissions and the final notice to that
reader. It avoids timing the long serial audience proof, but is not an all-user
arrival timestamp. Selected runs took 4.773 / 4.769 seconds, versus 4.671 seconds
for Control 3: roughly 2% slower bulk completion for much faster Close/control
responses. Earlier fixture revisions lack this added post-event timestamp; raw
receipts preserve their exact hashes.

## Mixed departures and all-user observations

Four two-second steady phases used 100,000 private-probe users, 8,192 multicast
readers, 32 saturating private pairs and eight paced background churn workers.
One broadcast per window and a room-member join/retirement after every window
exercise retirement while the private probes run. There are no inspectors in
this targeted mode. Probe rooms are phased across each period, avoiding a
generator-made single burst.

Every run completed all 16 offered windows, 131,072 multicast deliveries,
16 membership cycles, 512 background churn cycles and 800,000 private-probe
observations. The exact output contains 100,000 private rows with eight samples
each and 8,192 multicast rows with 16 samples each. All exits were `[0,0,0]`;
final resource gauges and copy/pressure/error counters were zero.

The percentiles below rank each user's **observed maximum**, not a stable
per-user p99. Intended-send maxima additionally include generator scheduling.

| Run | CPU, s | 95th percentile of private users' maxima, ms | Worst private observation, ms | Worst intended-send private observation, ms | Worst multicast observation, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.39 | 4.214 | 8.018 | 9.210 | 17.991 |
| Selected 1 | 9.34 | 2.477 | 4.497 | 7.361 | 20.894 |
| Selected 2 | 9.36 | 1.949 | 4.301 | 5.599 | 18.201 |
| Control 2 | 9.39 | 4.586 | 8.903 | 9.877 | 19.656 |

Private-probe tails improved in both selected runs. Multicast ranges overlap,
and some selected multicast observations were slower; this establishes no
multicast latency gain. Median private-user maxima also overlap. Saturating
private throughput varied: controls delivered 1.526 / 1.323 million messages/s;
selected runs delivered 1.230 / 1.424 million. The adopted benefit is reduced
departure-related response stalls, with the recorded CPU and bulk tradeoffs.

## Verification and reproduction

All 216 Rust tests, 66 JavaScript tests, formatting, Clippy warnings denied,
vendor provenance and the Rust/CBOR product audit passed. The new budget
regression checks zero and differently sized turns, exact visit bounds, FIFO
across those turns and final release of all queued credit. Existing tests select
their own explicit budgets. Native follow-ups passed the 100,000-observer,
eight-departure matrix under 2 MiB credit, the two-worker one-slot queue check,
and the cross-room local-pressure fallback fixture.

For the native burst, run `scripts/observer-barrier.py` with `--audience 100000
--departures 64 --server-cpus 0-7`, pinning the client to separate physical cores.
The receipt includes per-source Close samples and process snapshots before and
after the complete matrix.

For the mixed comparison, use the standard frozen-client recipe with:

```sh
python3 scripts/mixed-capacity.py --server /absolute/path/to/frozen/war-relay \
  --capacity-client /absolute/path/to/frozen/capacity \
  --sidecar /absolute/path/to/frozen/inspection --users 100000 --seconds 2 \
  --readers 8192 --scope room --pairs 32 --inspectors 0 --window 1 \
  --fanout-churn-every 1 --churn-workers 8 --churn-cycles-per-second 32 \
  --fanout-windows-per-second 8 --pin-client-workers --exact-fanout-latency \
  --output artifacts/mixed-departures.json
```

Adjust masks for the host. Control: commit `af638f1`, executable SHA-256
`34bd5cc087e2d0bd87181d449b3d014baf2eeb9e0d8598f1a98cce5ca9769c90`.
Selected executable SHA-256:
`912f73e9c146b75273d8b381b38eee98bebd6c60daa0979accabd5be6812b754`.
Copied source snapshots, build receipts, diagnostic logs, raw matrices and plot
inputs preserve the measured versions. Finite loopback tests establish neither
a sustained all-user latency SLO nor equivalent results on another machine.
