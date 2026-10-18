# Reject batched completed-write credit release

Keep the existing per-envelope release path selected. A correct RAII prototype
coalesces completed reservation refunds, but two short workload screens do not
show a repeatable fanout/latency benefit. No native runtime source, public API,
queue representation or running service changes are selected.

## Prototype and accounting contract

Each queued envelope currently refunds its per-peer and global byte reservation
when dropped. A vectored write can complete several envelopes, so the prototype
accumulates refunds belonging to the same budget and performs one combined
release at the end of the bounded write callback. Global admission remains an
exact atomic operation; no sharded/approximate total or larger limit is used.

The envelope's owned budget becomes an optional pointer. Completion moves that
owner into a stack accumulator before the envelope is dropped; the frame/body
retires normally. A changed budget owner flushes the previous group. RAII Drop
refunds any remaining completed credits on every scope exit, including unwind.
Partial frames keep their original envelope guard, and error/cancellation paths
retain ordinary per-envelope release. Refunds are conservative: completed credits
may remain charged until the callback ends, never released ahead of completion.
The private synchronous submission path is unchanged.

The invariant tests verify conservative charging until accumulator Drop, pending
reservations surviving peer retirement, mixed budget ownership and unwind.
The existing real-socket/probe tests independently verify borrowed body pointers,
FIFO, partial writes, cancellation and exact outstanding charges after each
write attempt. The changed private representation also adapts an existing
strong-owner-count assertion; it does not weaken that assertion.

All 80 existing clean library tests plus three targeted accounting tests and one
ephemeral layout probe pass (**84 total**). Clean library/native Clippy with
`-D warnings` passes. The measured host layouts remain Frame 48 bytes, Envelope
56 bytes and Pending 80 bytes; the stack accumulator is 16 bytes. The optional
pointer uses its niche, with no per-connection metadata growth or new allocation.
There is no unsafe code or payload copy.

Both release binaries come from sequential clean `581e84b` builds in the same
project and Cargo target paths. Only `relay.rs` and `server/socket.rs` differ.
The candidate was rebuilt after its final source/test formatting so receipts
identify that exact native binary. The root's pre-existing user modifications
were excluded and left byte-identical. Source prototypes exist only in the
disposable snapshot and archived patch.

## Mixed private/fanout screen

Four unprofiled two-second offers use control/candidate/candidate/control order:
64 private pairs, 2,048 fanout readers, four messages per checked fanout window,
existing 1 KiB values, no idle listing rooms, inspectors or churn. Four native
workers use CPUs 0–3; eight client workers use 4–7,20–23. Every worker is pinned
individually and clients use different physical cores from the server. Full
setup and checked departure cleanup (batch 32) are outside measurement. No
builds, profiles or other benchmarks overlap timed traffic.

| Trial | Variant | Private million messages/s | Fanout million deliveries/s | CPU-s / million combined deliveries | Private p99 histogram upper ms | Worst fanout window ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Control | 1.133 | 0.891 | 1.272 | 2.097 | 13.910 |
| 2 | Batched credits | 1.274 | 0.879 | 1.207 | 2.097 | 13.626 |
| 3 | Batched credits | 1.468 | 0.573 | 1.277 | 2.097 | 15.159 |
| 4 | Control | 1.073 | 0.808 | 1.346 | 4.194 | 13.309 |

The first candidate has a lower observed CPU/delivery cost. The second shifts
work strongly toward private traffic and away from fanout, so it does not
repeat the same gain. Unpaced mixture changes prevent interpreting an aggregate
number alone as an algorithmic improvement. Neither trial establishes uniformly
better response latency.

## Fanout-only mechanism screen

A second matched control/candidate/candidate/control sequence removes private
pairs and uses 16-message fanout windows. All other placement, audience, value,
duration and teardown settings remain. This gives the accumulator a larger
possible completion batch without the changing private/fanout mixture.

| Trial | Variant | Fanout million deliveries/s | CPU-s / million deliveries | Fanout p99 histogram upper ms | Worst fanout window ms |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | Control | 1.768 | 1.116 | 33.554 | 24.813 |
| 2 | Batched credits | 1.697 | 1.156 | 33.554 | 23.420 |
| 3 | Batched credits | 1.716 | 1.102 | 33.554 | 26.301 |
| 4 | Control | 1.809 | 1.152 | 33.554 | 21.469 |

Candidate fanout rates are lower in both observations, CPU ranges overlap and
latency does not consistently improve. These results do not justify the extra
owner/accumulator bookkeeping. The experiment is rejected without a longer or
100,000-user follow-up. This is not proof that refund batching can never help,
or that a particular branch/refcount operation uniquely caused the result.

All eight runs pass complete data/sender/FIFO matrices, exact protocol counters,
normal Close/departure, affinity and descriptor/resource retirement checks.
Error/pressure/rejection counters remain zero. Final connection/session/queue/
output-capacity/read-buffer/variable gauges and routing-copied-byte count are
zero; only the global channel remains. Offered traffic is unpaced and completed
amounts vary. Histogram figures are bucket upper bounds, not exact quantiles.
No trial establishes long-run per-user p99, production capacity or WAN/TLS behavior.

## Reproduce and audit

```sh
python scripts/inspection.py \
  --server artifacts/candidates/credit-batch-control/war-relay \
  --client target/release/examples/inspection \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers --output artifacts/credit-reproduction.json
```

For the fanout-only screen use `--pairs 0 --fanout-window 16`. Use fresh output
prefixes and never time variants concurrently. Masks describe this host. The
frozen rejected native candidate is under `artifacts/candidates/credit-batch-direct`.
It is not a selected/default server executable.

To reconstruct source, extract clean `581e84b` and apply `raw/candidate.patch`.
The patch includes targeted tests and the cfg(test)-only layout probe. Raw
receipts are losslessly gzip-compressed. Build/test/lint logs, `builds.json`,
`summary.json` and `SHA256SUMS` preserve the complete experiment. Run `verify.py`
for offline identity, arithmetic and retirement checks; it does not replay
network traffic. Root user changes and native/bridge/static services remain
unchanged, and no new worktree or deployment was needed for an unselected change.
