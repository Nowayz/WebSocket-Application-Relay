# Reject one-pass header preparation; expose shorter verified teardown

Selected tooling change `78d73a4` exposes the inspection client's existing
`--fanout-close-window` through `scripts/inspection.py`. Its default remains
one, so old commands and older clients receive no new flag. Values 1–32 match
the client's existing bound; this fixture uses its default 64-message queue.
Explicit 32 batches departure draining outside the measured phase. It preserves
normal closes, ordered notifications, exact counters and resource retirement.
It adds no server behavior, quota bypass or larger timeout.

The native optimization screened alongside this tooling change is **rejected**.
No native runtime source or selected live executable changed.

## Profile and candidate

The first two-second profiled run used 8,192 fanout readers, 64 private pairs,
four native workers and eight client workers. Private traffic saturates and
fanout sends four messages per verified window, waiting for every reader.
There are no inspectors, idle listing rooms or churn. Server CPUs are 0–3;
client workers use 4–7,20–23 and are individually pinned. Native workers are
also individually pinned. Bodies remain shared 1 KiB CBOR byte-string values.

Traffic completed, but one-at-a-time departure cleanup exceeded the unchanged
`seconds + 75` outer deadline (77 seconds for this case). The disposable client
was killed and the disposable server exited normally. This receipt is retained
as a failure, not accepted stability or performance evidence. Its initial
runner source and text profile are also retained.

After exposing the existing departure batch option, the fresh profiled case
with `--fanout-close-window 32` passed all gates. Readiness was at 0.446 seconds,
the measured gate at 2.655 seconds and complete checked retirement at 41.410
seconds. Establishment/teardown are additional to the two-second traffic offer.
This is a shorter complete test, not a claim that its whole invocation lasts
two seconds or that the timed relay operation became faster.

The accepted profile reports zero lost samples and attributes 9.52% of self
user-cycle samples to `write_batch`. Profiles change execution conditions and
are excluded from ordinary timing comparisons. The text report is archived;
raw recordings remain local, with identities in `builds.json`.

The control builds headers for up to 16 queued frames, then walks those frames
again to build borrowed scatter/gather slices. The candidate combines those
passes using disjoint mutable header slots zipped with queue entries. It removes
the temporary length array and stops preparing headers at the byte quantum.
Both retain 16-frame/32-iovec and existing byte limits, stack-owned headers,
borrowed immutable bodies, partial-write offsets and FIFO queue retirement.
There is no unsafe code, new allocation or new scheduling policy.

Both release binaries were built sequentially from clean `239a6b8` snapshots
in the same project/target paths. Their only native source difference is the
archived `src/server/socket.rs` change. Pre-existing user overlays were excluded
and verified unchanged. All eight existing socket tests pass on the candidate,
including short writes, FIFO, retained-body ownership and charge retirement.
Passing these tests does not establish a performance win.

## Short ordinary screen

Four unprofiled two-second cases use control/candidate/candidate/control order.
The screen reduces the audience to 2,048 readers to keep exhaustive retirement
short; complete runs take about four seconds. The same 64 private pairs,
1 KiB values, four-message fanout windows, disjoint CPUs and pinning remain.
Explicit departure batches of 32 are identical across all four trials and run
outside measurement. No builds, profiles or other benchmarks overlap traffic.

| Trial | Variant | Private million messages/s | Fanout million deliveries/s | CPU seconds / million combined deliveries | Private p99 histogram upper ms | Worst fanout window ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Control | 1.158 | 0.723 | 1.368 | 2.097 | 17.171 |
| 2 | One pass | 1.454 | 0.604 | 1.269 | 2.097 | 18.835 |
| 3 | One pass | 1.120 | 0.720 | 1.387 | 4.194 | 22.813 |
| 4 | Control | 1.260 | 0.765 | 1.290 | 2.097 | 14.428 |

CPU/delivery ranges overlap. The first candidate's aggregate gain also hides a
lower fanout rate; its repeat loses that private-throughput advantage and moves
to a worse private-latency bucket. Candidate fanout maxima are worse in both
observations. This finite screen does not establish a universal regression,
but it provides no defensible reason to select the change for low, consistent
latency. There is no follow-up multi-hour test or 100,000-user run for a candidate
that failed this screen. Original native source remains selected.

All four ordinary runs pass exact content/sender/FIFO, protocol counters,
normal departure notifications, zero error/pressure/rejection counters and
complete retirement. Final connection/session/queue/output-capacity/read-buffer/
variable gauges and routing-copied-byte counter are zero; only the global
channel remains. Fanout and private traffic are unpaced, so delivered amounts
and their mixture vary. Normalized CPU is an observation, not a fixed-work
causal estimate. These tests do not establish long-run tails or WAN/TLS capacity.

## Tool verification and reproduction

The successful 8,192-reader profile and all four ordinary runs exercise explicit
batch 32. A separate one-second 128-reader/one-pair test passes with default
batch one and confirms the client command contains no new flag. CLI values
zero and 33 reject before process launch; Python syntax and diff whitespace
checks pass. The outer deadline remains unchanged. No new native test or
runtime configuration was introduced merely to support the experiment.

```sh
python scripts/inspection.py \
  --server artifacts/candidates/socket-header-control/war-relay \
  --client target/release/examples/inspection \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers --output artifacts/header-screen-reproduction.json
```

Use fresh output names. The profile recipe substitutes 8,192 readers and adds
`--profile`; omit that flag for ordinary measurements. Do not time variants
concurrently. The frozen candidate is under `artifacts/candidates/socket-header-direct`;
it is intentionally unselected. The CPU masks describe this host.

Receipts are losslessly gzip-compressed; console output, profiles, source
variants, build/test logs and CLI checks are archived. `summary.json` extracts
ordinary results, `builds.json` pins binary/local-profile identities and
`SHA256SUMS` covers the archive. Run `verify.py` for offline integrity,
source-attribution and gate checks. This does not replay network traffic.
All three pre-existing source modifications remain byte-identical, and the
selected native, bridge and static-server processes remain unchanged.
