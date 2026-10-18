# Notification frequency and counter-layout screen

Keep the existing notification and counter layouts. The two proposed runtime
changes did not earn selection. Retained source change `c4ba653` instead corrects
`war_queued_bytes`'s description and documents its exact admission-accounting
role. No counter value, quota, wakeup, atomic operation or ordering changed.

## Already-notified frequency

An isolated diagnostic preserves the original `notified.swap(true, Relaxed)`
and adds counters for calls and previous-true outcomes. Two seconds of traffic
with 64 private pairs and 2,048 fanout recipients recorded **1,375,861 calls and
zero previous-true outcomes**, subtracting the ready-gate snapshot from the
measured-gate snapshot. Setup and exhaustive departure cleanup are excluded.
Exact content/FIFO, counter and resource-retirement checks pass.

A proposed load-before-swap guard would save an atomic read-modify-write only
when the flag was already true. This run provides no such opportunity, so the
extra branch/load is not implemented. Socket output already requests ordinary
notifications when the queue first becomes nonempty. This observation does not
prove duplicates can never occur in another workload or race.

The diagnostic adds global atomic counters and changes layout; its throughput
and timing are **not** performance results. Its source, binary hash and receipt
are retained separately. No new notification telemetry enters production.

## Counter layout candidate

A layout probe found one `Arc<Metrics>` allocation where receive and delivery
counters shared a 64-byte cache line. The measured host coherency-line size is
64 bytes. This is a possible layout collision, not a PMU measurement of false
sharing in the ordinary native runs or a claim about every allocation.

The candidate uses fixed field order and zero-sized 64-byte alignment markers
before the receive and delivery groups. All existing counter types, operations
and orderings remain ordinary atomics. No shard, approximate aggregation,
per-frame branch, per-connection state or dependency is introduced.

| Layout probe | Control | Candidate |
| --- | ---: | ---: |
| `Metrics` size | 288 bytes | 320 bytes |
| Alignment | 8 bytes | 64 bytes |
| Observed allocation address modulo 64 | 16 | 0 |
| Incoming-counter offset | 64 | 64 |
| Delivery-counter offset | 96 | 128 |
| Delivered-byte counter offset | 104 | 136 |
| Queue-reservation counter offset | 168 | 200 |

The candidate guarantees separate receive/delivery cache lines on this host.
The metrics object is allocated once per relay, although allocation/alignment
padding can exceed the logical structure-size difference. No per-user object
size changes. The probe is ephemeral and not a shipped unit test.

Both ordinary native binaries come from clean `7877db4` snapshots in the same
project/target paths. Their native source difference is only the metrics layout.
Pre-existing user overlays are excluded. All 80 existing library tests plus the
layout probe pass, and clean library/native Clippy with `-D warnings` passes.

## Short whole-relay comparison

Four unprofiled trials use control/candidate/candidate/control order. Each offers
two seconds of saturating 1 KiB-value traffic with 64 private pairs, 2,048 fanout
recipients and four messages/window. There are no inspectors, listing rooms or
churn. Four native workers use CPUs 0–3, eight client workers use 4–7,20–23,
all individually pinned. Physical client/server cores are disjoint. Setup and
checked departure cleanup (batches of 32) are outside measurement. No builds,
profiles or other benchmarks overlap traffic.

| Trial | Variant | Private million messages/s | Fanout million deliveries/s | CPU-s / million combined deliveries | Private p99 histogram upper ms | Worst fanout window ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Control | 1.176 | 0.898 | 1.247 | 2.097 | 11.716 |
| 2 | Separated groups | 1.174 | 0.666 | 1.385 | 2.097 | 15.275 |
| 3 | Separated groups | 1.234 | 0.613 | 1.380 | 2.097 | 18.059 |
| 4 | Control | 1.155 | 0.702 | 1.377 | 2.097 | 19.101 |

The control return run itself varies materially. Candidate CPU/delivery does
not improve and both candidate fanout rates are below the controls. Private
latency buckets are identical and maxima overlap. There is no repeatable
whole-workload gain to justify imposing the layout, so it is rejected without
a larger/longer follow-up. This does not establish a universal effect of cache
padding or identify a unique cause for variation. Unpaced work amounts/mixes
vary; normalized CPU is not a fixed-work causal estimate.

Every trial passes exact body/sender/FIFO, protocol-counter and normal-departure
checks. Error/pressure/rejection counters are zero. Final connection/session/
queue/output-capacity/read-buffer/variable gauges and routing-copied-byte count
are zero; only the global channel remains. These finite loopback screens do
not establish long-term tails, WAN/TLS behavior or 100,000-user performance.

## Retained accounting clarification

`queued_bytes` participates in `Peer::reserve`'s global admission CAS and in
`RoomFanout::begin`'s charge for captured membership storage. It is not merely
an observational payload gauge. The old HELP text omitted that membership
charge and its admission role. The selected source now says:

> Logical output reservations used for global admission, including in-flight
> messages and active fanout membership snapshots; shared bodies may be charged
> per recipient.

A source comment makes the same exact-accounting contract explicit. The value
is logical reserved work, not a direct measurement of resident payload memory.
Sharding/approximating that admission total would require a separate correctness
design; neither experiment weakens it.

A fresh disposable build of the selected description-only source was checked
through its actual HTTP endpoint: unauthenticated access returns 401, authorized
access returns 200 with the updated HELP line and an unchanged zero gauge, and
normal process shutdown exits zero. No broad test suite was repeated for a
comment/HELP-text change; build, endpoint, formatting and whitespace checks pass.
The running manual native service was not restarted and retains its older HELP
text until a future selected deployment.

## Reproduce and audit

Each raw receipt pins commands, binary/client/runner hashes, CPU assignments,
phase counters and cleanup. The ordinary recipe is:

```sh
python scripts/inspection.py \
  --server artifacts/candidates/metrics-layout-control/war-relay \
  --client target/release/examples/inspection \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers --output artifacts/metrics-layout-reproduction.json
```

Use fresh names and never time variants concurrently. The rejected layout
binary is under `artifacts/candidates/metrics-layout-direct`; the frequency
probe is under `artifacts/candidates/notify-frequency`. The selected accounting
HELP check uses `artifacts/candidates/metrics-accounting`. CPU masks describe
this host. None of these paths replaces a live server.

Receipts are losslessly gzip-compressed. Exact experimental/selected source,
layout probes, build/test/lint logs, `builds.json`, `summary.json` and checksums
preserve attribution. Run `verify.py` for offline hashes, diagnostic deltas,
variant identities and retirement gates; it does not replay network traffic.
All three pre-existing source modifications remain byte-identical. Selected
native/bridge executables and the static-server process remain unchanged.
