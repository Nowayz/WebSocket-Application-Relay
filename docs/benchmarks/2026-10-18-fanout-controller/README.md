# Bounded acknowledgement collection and fanout-controller timings

Selected benchmark-client change: `57ba85c`. The fanout load generator drains
acknowledgements with `mpsc::Receiver::recv_many` into one reused vector bounded
by reader count. Every index still undergoes bounds and duplicate checks. A
single five-second acknowledgement-collection deadline replaces a fresh
five-second deadline per index. Reader failure, channel closure and malformed
acknowledgements remain errors. There is no relay-server or wire-protocol change.

The earlier helper could keep collecting far beyond five seconds if a missing
reader set trickled progress. The new deadline covers collection **after the
publisher finishes sending**; it does not bound publisher writes, setup,
teardown or the entire invocation. A focused test sends acknowledgements two
seconds apart and proves that progress cannot restart that deadline.

One `usize` vector is reserved once per controller (64 KiB for 8,192 readers on
this host), then reused across message and departure windows. It does not hold
message bodies or introduce per-window allocation. No new dependency or server
configuration was added.

## Optional stage diagnostics

Existing exact fanout diagnostics (`--exact-fanout-latency` in the mixed runner)
now also emit `fanout_controller_timing` in the sidecar's measured event:

- `ready_ns`: controller setup finished, relative to the shared sidecar phase start.
- Per window: `scheduled_ns`, `wake_ns`, `sent_ns`, `acknowledged_ns`, `completed_ns`.
- Send includes optional membership join and publisher writes; acknowledgement
  waiting includes checked relay/network/reader-task work and collection.
  Completed additionally includes optional membership departure handling.

All timestamps use the same monotonic process clock and phase origin. One row
per paced window is reserved using the existing finite exact-sample bound;
recording refuses overflow rather than growing storage during traffic. It adds
no per-delivery timestamp or allocation beyond the previously enabled exact
latency samples. With exact diagnostics disabled, the field is null, timing
storage is absent and the extra send/acknowledgement clock reads are skipped.

The run helper accepts the measurement's start/end as one range so the origin
is explicit. The initial timing-only probe used equivalent separate arguments;
its exact source is archived as measured rather than silently rewritten.
Neither version changes the timer's epoch, skip policy, window barrier, offered
rate or observed delivery matrix.

## Short component evidence

An ephemeral ignored release test prequeues 8,192 acknowledgement indices,
validates all readers, and compares the old helper with the batched helper in
old/batch/batch/old order on CPU 7. Each variant warms up ten windows, then
records nine batches of ten windows. Channel, seen flags and batch storage are
preallocated; queuing is outside the timed collection. The entire screen took
0.23 seconds. Its exact source and output are archived, not shipped in the
example's normal test suite.

| Collection variant | Median batch time per window |
| --- | ---: |
| Old, first | 535.0 us |
| Batched, first | 25.84 us |
| Batched, repeat | 25.73 us |
| Old, return | 537.8 us |

This is roughly 95% less queued-index collection time. It is a prequeued
controller screen, not a network latency measurement. Even a 0.51 ms saving
across 96 windows is only about 49 ms, small relative to the full sidecar's
18+ CPU-seconds. Do not translate this ratio into a relay performance claim.

## 100,000-user diagnostics and acceptance

The same frozen native executable is used before and after. Both sidecars run
against 100,000 private users, 8,192 fanout readers, eight churn workers at 4 Hz,
and two CBOR inspectors. Traffic is offered for six seconds: 4 probes/private
user/second and four fanout messages/window at 16 windows/second. Exact fanout
and private stage diagnostics are enabled equally. Server workers use CPUs
0–7; capacity uses 8–11,24–27 and sidecar 12–15,28–31; all 16 client workers
are individually pinned. Setup, output and exhaustive retirement are outside
the offer. No builds, profiles or other benchmarks overlap measured phases.

| Observation | Timing-only old helper | Selected batched helper |
| --- | ---: | ---: |
| Controller ready after phase start | 9.83 ms | 12.31 ms |
| Median timer wake delay | 1.07 ms | 1.37 ms |
| Median send span | 2.23 ms | 2.28 ms |
| Median acknowledgement span | 47.26 ms | 47.08 ms |
| Maximum acknowledgement span | 79.95 ms | 80.70 ms |
| Whole iterations over the 62.5 ms offer period | 11 | 4 |
| Completed/offered fanout windows | 96/96 | 96/96 |
| Sidecar CPU-seconds | 18.71 | 18.47 |
| Native CPU-seconds | 20.42 | 20.88 |
| Population p99 of private users' observed worst | 9.50 ms | 13.61 ms |
| Worst observed fanout window | 81.91 ms | 82.70 ms |

Startup was below one offer period in these trials; it does not explain the
previous run's missing window. The acknowledgement interval dominates this
controller's iteration time, but it includes the relay, kernel, reader scheduling
and validation. The timing cannot attribute all of that span to any one layer.
Some iterations exceed a period while total offered work still completes,
because faster iterations allow progress between stalls. No missed window was
reproduced here; its earlier cause remains unresolved.

The full-run figures do not establish a general latency or CPU win. Private
tails were worse in the selected trial, fanout maxima overlap, and one pair is
insufficient to attribute a code regression or improvement. Retention is based
on the bounded deadline, reduced component bookkeeping and simpler timer
management, not a claim that the native relay became faster.

Each large run verifies 2.4 million timed private probes and 3,145,728 timed
fanout deliveries, with exact body/sender/FIFO, counters, normal Close/departure
and descriptor/resource retirement. Routing copied bytes and error/pressure/
rejection counters remain zero. Final connection/session/queue/output-capacity/
read-buffer/variable gauges are zero; only the global channel remains. Each
private user has 24 observations, so its nearest-rank p99 is its observed max.
These finite loopback diagnostics do not establish long-term p99 or deadlines.

## Verification and integration

All seven example tests pass in both the clean candidate and actual workspace.
They include non-primary reader/global-observer corruption and departure
rejection, unique/out-of-order index handling, buffer reuse, duplicate/invalid/
closed acknowledgements, and the trickle-progress deadline test. Tests take
five seconds, dominated by that intentional timeout. Clean example Clippy with
`-D warnings`, formatting and diff whitespace checks pass.

A two-second 1,600-user/512-reader initial diagnostic check passes 32/32 windows.
A matching selected-client run with exact diagnostics disabled also passes all
gates and 32/32 windows, and verifies a null timing field. The current default
`target/release/examples/inspection` now matches the frozen verified client;
its previous hash was checked before atomic replacement. No native, bridge or
static-server process was restarted, and all three pre-existing user-modified
source files remain byte-identical.

## Reproduce and audit

```sh
python scripts/mixed-capacity.py \
  --server artifacts/candidates/page-partition/war-relay \
  --sidecar artifacts/candidates/fanout-batch-client/inspection \
  --capacity-client artifacts/candidates/latency-stages-client/capacity \
  --users 100000 --seconds 6 --readers 8192 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency --latency-stages --pin-client-workers \
  --output artifacts/fanout-controller-reproduction.json
```

Use a fresh output prefix. For a quick check use 1,600 users, 512 readers and
two seconds. Omitting exact fanout latency disables the extra timing records.
The CPU topology is host-specific; preserve separation when adapting the test.

Receipts, CSV matrices, private window timelines and console logs are losslessly
gzip-compressed. Decoded hashes match receipts. Exact before/after client
sources, selected build metadata, test/lint logs and the component screen are
archived. `summary.json` extracts all observations rather than selecting the
best run. `SHA256SUMS` protects archive contents; `verify.py` checks decoded
identities, sample counts, stage ordering and resource gauges offline. It does
not replay network traffic or establish causal confidence for timing differences.
