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
lock, queue, per-peer field, allocation policy or configuration flag. At this
revision the deferral threshold stayed at 1,024 recipients; the cutoff follow-up
below changes that threshold too. Deadlines and backpressure policy retain their
behavior. A recipient-count bound is not an elapsed-time deadline under OS
preemption or memory stalls.

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


## Deferral cutoff follow-up

The initial scheduling change shortened deferred turns but left departures of
up to 1,024 recipients synchronous under the registry write lock. Native
publication now uses the same **128-recipient budget** as advancement. Both
ordinary rooms and eligible disconnect audiences defer above that boundary.
The constant is shared with the native worker, while live message fanout stays
at 1,024. Older pending jobs still force ordered handling across the cutoff.

Outputs that cannot drive retirement are checked before capturing a room view,
avoiding a useless recipient-vector allocation before synchronous delivery.
Reservation refusal retains the existing ordered best-effort fallback. No new
lock, queue, per-peer field, payload copy or external configuration is added.
This is the normal native publication cutoff, not a maximum on fallback work.

A finite diagnostic build confirmed the remaining synchronous cost. With 100,000
connections, 1,023 eligible recipients and 64 singleton-source closes, membership
removal held the registry lock for 13.445 ms in total, inside a 17.431 ms burst.
Registry acquisition itself totaled 0.005 ms. Diagnostic logging is confined to
its copied source and supplies no production comparison row.

### Population independent of the audience

`scripts/observer-barrier.py --idle-users` now separates connection population
from disconnect fanout. Idle users occupy ordinary rooms of up to 1,024 members
and subscribe to no observation class. Startup verifies the entire declared
physical/session count, and shutdown checks every retained connection.

The boundary screen uses 1,022 audience observers, the tested observer, 64
sources, one independent echo witness and 98,912 idle users: **exactly 100,000
connections** before the event. Peak room membership stays within 1,024. Each
accepted case checks 68,539 notices, FIFO, unsubscribe/re-subscription, zero
residual queued credit, zero recorded routing-copy/error counters and all 99,934
retained shutdown Close `1001` frames. The other original participants close
normally during the fixture. Every process exits without a forced kill.

| Run | Close burst through revoke ACK, ms | Median Close RTT, ms | Worst Close RTT, ms | Worst short-event witness RTT, ms |
| --- | ---: | ---: | ---: | ---: |
| Control 1 | 17.346 | 0.283 | 0.624 | 0.501 |
| Control 2 | 17.219 | 0.289 | 0.633 | 0.349 |
| Final selected 1 | 13.554 | 0.197 | 0.820 | 0.482 |
| Final selected 2 | 13.275 | 0.174 | 0.716 | 0.277 |

The final build improves total/median Close response; worst individual Close
and witness ranges do not uniformly improve. Completed-work process CPU is
0.07 / 0.08 seconds for controls and 0.09 / 0.09 for selected runs. These short
CPU intervals are quantized in 10 ms ticks, so they do not support a precise
percentage efficiency claim. The first-audience completion timestamp in this
small case follows the fixture's deliberate post-event sampling pause and is
not evidence of packet-arrival latency. An earlier literal-cutoff prototype
passed too; its receipts remain exploratory rather than final-build timings.

### Frequent small-room departures with private probes

The second screen uses the same frozen clients and 100,000 private-probe users,
plus 1,022 stable room readers, 32 saturating private pairs and paced background
churn. One broadcast and a member retirement occur per window, at 32 offered
windows/s. The temporary member brings peak room size to exactly 1,024; its
removal leaves 1,023 recipients, exercising the changed path. There are no
inspectors in this targeted mode.

Each two-second phase completes all 64 offered windows, 65,408 multicast
observations, 64 membership cycles, 512 background churn cycles and 800,000
private-probe observations. Each private user supplies eight samples; each
multicast reader supplies 64. All process exits, exact messages, departures and
final resource/copy/pressure/error gates pass. The table ranks users' observed
maxima; it is not a long-term per-user p99 estimate.

| Run | CPU, s | 95th percentile of private users' maxima, ms | Worst private observation, ms | Worst intended-send private observation, ms | Worst multicast observation, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.37 | 1.458 | 4.335 | 5.981 | 2.642 |
| Control 2 | 9.38 | 1.599 | 5.326 | 9.749 | 3.553 |
| Final selected 1 | 9.46 | 1.524 | 3.733 | 8.451 | 2.752 |
| Final selected 2 | 9.45 | 1.619 | 4.174 | 7.450 | 3.015 |

The worst private observations are lower in both selected runs. Broader private
percentiles, intended-send maxima and multicast results overlap, so no broad
percentile or multicast improvement is established. Private throughput varies:
controls deliver 1.614 / 1.345 million messages/s; selected runs 1.612 / 1.450
million. The cutoff is retained for shorter aggregate/median Close handling and
the observed private-response peak reduction, with modest measured CPU cost.
These finite screens do not establish a universal latency bound.

All 219 Rust tests, 66 JavaScript tests, formatting, Clippy warnings denied,
vendor provenance and the Rust/CBOR product audit pass. New regressions check
128/129 recipients in ordinary rooms and global audiences, FIFO and refusal at
the new cutoff, and no captured room view for unsupported outputs. Native
follow-ups cover 100,000 observers/eight departures under 2 MiB credit, one-slot
queues, cross-room pressure fallback and the new small idle-population CI case.

Reproduce the fully populated boundary screen with `--audience 1022
--idle-users 98912 --departures 64`. For the mixed recipe above, change
`--readers` to `1022` and `--fanout-windows-per-second` to `32`. Use the same
frozen clients, CPU masks and fresh receipt paths before and after.

Control: commit `6bcb8bb`, executable SHA-256
`912f73e9c146b75273d8b381b38eee98bebd6c60daa0979accabd5be6812b754`.
Final selected executable SHA-256:
`b4fde7a2012be2279121a7a6e68491a14e0255343b69bbcc3da4c2f9355dcb5b`.
Source snapshots, raw matrices, diagnostic logs and earlier prototype records
preserve the complete experiment. Existing synchronous fallback remains a
separate latency limit when a deferred reservation cannot be funded.
