# Short reproducible performance work

Measure the complete workload before changing it. Preserve payload bytes,
per-recipient FIFO, permission/acknowledgment boundaries and memory accounting.
A component win is not a complete-relay win. Optimize latency alongside CPU,
throughput, copied bytes and retained memory; reject changes whose tradeoffs
conflict with consistently responsive users.

## Current runtime

Mio workers own decoding and timers. Private sends retain their immediate
borrowed path. Multicast output uses shared bodies and bounded owner-worker
vectored writes. Large native room, global and observer publications advance
by 1,024 recipient visits per turn; departure jobs use 128 visits. The
[cutoff follow-up](reports/retirement-turns.md#deferral-cutoff-follow-up)
measures the normal-sized-room boundary at a 100,000-connection population,
and the [retirement scheduling measurements](reports/retirement-turns.md)
record the CPU/bulk-delivery tradeoff of the shorter departure turn. Expiry
discovery is also bounded. Scalar control replies fit the inline frame prefix.
No alternative allocator or native numerical library is selected by default.
The locking model, the per-room departure queues and the pressure policy that
replaced the single registry lock and single retirement FIFO those reports
describe are in [architecture.md](architecture.md).

Workers service separate fresh-readiness and buffered/output token queues.
Each phase has a 4,096-visit routing allowance, finishing the ordinary 1,024-visit
source turn before yielding. Fresh service is capped at 1,024 connection turns;
buffered service at 1,024. Both receive progress each loop. This bounds aggregate
publication work without changing the source deferral cutoff or staging cost.
See the [worker-scheduling comparison](reports/worker-scheduling.md) for the response-time
benefit, burst CPU/batching tradeoff and limits of the allowance.

Pending disconnect jobs reuse an immutable observer snapshot. Its ownership
reserves membership storage once, and a weak cache retains no recipient
storage after the last job finishes. Ordinary room and message-observer views retain their conservative
per-publisher charge. See the [snapshot ownership measurements](reports/retirement-snapshots.md).

The observer-revocation barrier submits only the requesting observer's
notices. A publication sequence and per-peer watermark prevent deferred replay;
other recipients keep their normal turns. A barrier reuses its peer lookup across
jobs sharing the same immutable view. See the [lookup follow-up](reports/observer-barriers.md#shared-view-lookup-follow-up).
Queue-pressure fallback finishes only the older jobs of the queue being
bypassed; other queues retain their turns and reservations. See the
[fallback measurements](reports/retirement-fallback.md) and the
[measured event screen](reports/observer-barriers.md) before choosing the next bottleneck.

## Small checks before timing

```sh
cargo build --locked --release --bin war-relay --example capacity --example inspection
cargo test --locked --all-targets
bash scripts/check-js.sh
```

Freeze executables and source identities before comparisons. `scripts/build-snapshot.py`
rebuilds a copied Cargo source snapshot, refuses stale Cargo artifacts and records
source/executable digests. Use fresh output filenames and serialize builds and
measurements. The scripts refuse evidence collisions. Ordinary timings should
exclude profilers, fault injection and allocator preloads.

## Capacity and fairness

A small portability/CI check needs no fixed CPU masks:

```sh
python3 scripts/capacity.py --server target/release/war-relay \
  --client target/release/examples/capacity --users 256 --seconds 2 \
  --churn-workers 2 --workers 2 --client-workers 2 \
  --server-cpus '' --client-cpus '' --output artifacts/capacity-short.json
```

For the full population, use 100,000 users on a host with descriptor/memory
headroom and separate generator cores. Keep the steady phase short; population
setup, proof delivery to every session, CSV generation and complete cleanup still
take additional wall time. Inspect each user's samples and the worst events,
not just an aggregate percentile. Intended-send latency also includes client
scheduling lag; send-to-receive latency alone can hide an overloaded generator.

The mixed runner adds fanout readers, private traffic, inspection and churn:

```sh
python3 scripts/mixed-capacity.py --server /absolute/path/to/frozen/war-relay \
  --capacity-client /absolute/path/to/frozen/capacity \
  --sidecar /absolute/path/to/frozen/inspection --users 100000 --seconds 2 \
  --readers 8192 --scope room --pairs 32 --churn-cycles-per-second 32 \
  --fanout-windows-per-second 8 --pin-client-workers \
  --output artifacts/mixed-short.json
```

Its masks assume server CPUs 0–7 and physically separate client cores on the
original 32-logical-CPU host. Adjust for other hosts. `--scope observers` covers
global channel-message observers. Every offered fanout window must complete;
correctness alone cannot pass a missed offered-load gate. Normal runs check exact
messages, departures, all process exits and final resource gauges.

## Focused event and pressure checks

- `scripts/departure-fairness.py` checks a large departure alongside independent
  witnesses and verifies every expected notice. Optional `--queue-budget-bytes`
  is an explicit pressure diagnostic, not an ordinary performance setting.
- `scripts/fragment-fairness.py`, `scripts/fragment-waves.py` and
  `scripts/handshake-pressure.py` probe decoder/timer fairness.
- `scripts/accept-pressure.py` uses a child-only descriptor limit to check accept
  recovery, existing-client responsiveness, queued-flow recovery and shutdown.
- `scripts/shutdown-capacity.py --prime-broadcast` warms and verifies a room's
  recipient cache before checking every client's shutdown Close `1001` and TCP
  EOF. Compare with the unprimed case and keep the same shutdown grace.
- `scripts/retirement-fallback.py` stages one full-size echo in a deliberately
  blocked source's local output budget, publishes a large-room departure, then
  administratively disconnects the blocked source in another room. It verifies
  both healthy notice paths, an independent witness and every audience Close/EOF.
  One cancelled partial write is required for that deliberately blocked source;
  other error counters must remain unchanged. Use `--audience 100000` with separate
  client/server cores for the measured case, or 2,048 peers and two workers in CI.
- `scripts/observer-barrier.py` checks native unsubscribe ordering, re-subscription,
  an independent echo witness and every audience member's departure/Close matrix.
  Use `--audience 100000 --server-cpus 0-7` on the measured host, with the client
  pinned to separate physical cores. Default teardown serializes follow-up
  departures; `--burst-teardown` separately probes queue-pressure behavior.
  `--departures 8` queues distinct singleton-source departures before revocation.
  `--idle-users` adds connected non-observers in rooms of up to 1,024 members,
  so population and eligible fanout can be varied separately. Startup checks the
  entire declared population; shutdown verifies every retained connection.
  `--queue-budget-bytes 2097152` explicitly constrains credit without relaxing
  the full wire matrix; its pressure-counter flag reports refusals, not lost
  message counts. Successful delivery at that limit is the acceptance check.
- `scripts/observer-message-fairness.py` checks channel/private observer messages
  before unsubscribe acknowledgments, immediate re-subscription, independent
  witnesses, joins and every audience delivery. `--kind channel` uses private
  echoes; `--kind private` uses control replies to avoid making the witnesses
  publish additional fanouts. The [baseline report](reports/observer-messages.md) records
  the 100,000-observer stalls, profile, one-slot CI cases and timing limits.
  `--room-readers` adds same-room recipients to channel publications; combining
  it with `--queue-bytes 2048` checks single-body staging under a tight local
  budget. The [bounded-publication report](reports/observer-publications.md) records the
  implementation, ownership and remaining cold-view/barrier scan costs.
  `--publishers 8 --kind mixed --payload-bytes 8` overlaps singleton sources
  alternating channel/private messages while checking every source at every
  observer. The eight-byte marker keeps the burst small; the default single-source
  value remains 1 KiB. `--room-readers` requires one channel publisher.
  See the [concurrent-publication screen](reports/concurrent-observers.md) for capture
  proof, source-order limits, the manual barrier diagnostic and a rejected cache
  experiment whose component result did not establish a native eight-worker win.
- `scripts/inspection.py` measures CBOR command or HTTP inspection during checked
  private traffic, room multicast and churn. It has no alternate wire format.
  Its isolated `--fanout-global-broadcast` mode checks global publications,
  independent join/echo/Close responsiveness, every reader and exact cleanup.
  See [the global broadcast report](reports/global-broadcasts.md) for the short recipe,
  measured scheduling benefit and cold-view/observer limitations.
- `scripts/stability.py`, `scripts/retention.py` and `scripts/queue-waves.py`
  cover bounded retention and overload recovery. Choose short explicit phases.

Use each script's `--help` for required arguments and host affinity. Reuse the
same client binaries and workload settings before/after. Run a few short serial
control/candidate/candidate/control trials when placement variation matters.
Report ranges, failures and semantic differences. Do not infer language-wide
performance rankings or production guarantees from loopback diagnostics.

The full historical data and temporary comparison implementations are available
through Git history; [the concise history](reports/performance-history.md) records useful
results and exact retrieval paths. Current work compares Rust revisions.
