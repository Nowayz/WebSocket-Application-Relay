# Current performance guide

Use this page to find the selected implementation and choose a short test. The detailed [performance history](performance.md) and individual reports retain the evidence; experimental binaries and attractive isolated results are not automatically selected runtime changes.

## Selected implementation and evidence

| Component | Selected behavior | Evidence |
| --- | --- | --- |
| Native Rust relay | Mio readiness workers; immediate private sends; shared immutable multicast bodies; bounded vectored output; 1,024 ordinary-room recipient visits per source turn | [100,000-user mixed-load selection](benchmarks/2026-10-18-mixed-100k-fairness/README.md) |
| Legacy bridge | CBOR upstream, upstream TCP_NODELAY, 4 KiB initial upstream read buffer; direct single-buffer binary and text encoding; existing frame/message limits retained | [Delayed-ACK fix](benchmarks/2026-10-18-bridge-upstream-nodelay/README.md), [connection memory](benchmarks/2026-10-18-bridge-read-buffer/README.md), [binary encoding](benchmarks/2026-10-18-bridge-fused-encoding/README.md), [text encoding](benchmarks/2026-10-18-bridge-text-encoding/README.md) |
| Allocator | Existing system allocator; no default jemalloc preload | [Four-wave retirement/allocator screen](benchmarks/2026-10-18-churn-retention/README.md) |

The frozen native baseline is runtime revision `279c3c1`, SHA-256 `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`. The frozen bridge baseline is code revision `4c9113a`, SHA-256 `bb2392f53bd8fe1bdff47f0a8e790b366269400e259839565bdeb4573f9f8bf1`. These identify measured artifacts, not the repository's latest documentation commit. A new build must be identified by its own source and executable hashes.

The primary mixed-load comparison has 100,000 private users plus 8,192 fanout readers, churn and inspection. Selected Rust's median user p99 is 2.03 ms, versus 11.86 ms for the accepted modern C++ reference; sampled native RSS is about 115 MiB versus 70 MiB. Those are finite loopback observations under that report's exact workload, not universal language rankings or total machine memory requirements. Kernel sockets and generators consume additional memory. The original upstream C++ implementation is a separate reference with its original protocol; follow [the C++ comparisons](../cpp/BENCHMARKS.md) rather than assigning modern C++ results to it.

## Choose the smallest test that answers the question

| Question | Test | Measured phase |
| --- | --- | --- |
| Does a change hurt private users while a hot room broadcasts? | `scripts/mixed-capacity.py --seconds 6` with the full mixed-load options below | Six seconds; 24 observations/user at 4 Hz |
| Do repeated population bursts retire correctly? | `scripts/capacity.py --users 16384 --room-size 2 --waves 4 --seconds 1 --active-percent 10` | Four one-second traffic phases, with checked joins/closes between |
| Does bridge cancellation retire both legs while native traffic survives? | [Disposable shutdown probe](benchmarks/2026-10-18-bridge-shutdown/README.md) | Small joined/incomplete populations; current transport-cancellation policy |
| Did input/control handling change? | `scripts/input-corpus.py --workers 4 --clients 8` | Finite 776-case independent corpus |
| Do stalled upgrades or Ping-only prejoin peers release admission? | `scripts/handshake-pressure.py` and `scripts/join-pressure.py` | Fixed five-second HTTP / two-second join-deadline fixtures; [checked examples](benchmarks/2026-10-18-stalled-ingress/README.md) and [unacknowledged Close](benchmarks/2026-10-18-unacknowledged-close/README.md) |

Setup, proof scans and exhaustive departure/Close checks add wall time outside the measured phase. In particular, six seconds of offered mixed traffic does not mean a six-second complete test. Extend a test only when the result needs it; a short screen can reject a clear regression without pretending to prove a long-term tail guarantee.

To test a new build, build it deliberately from the source tree you intend to measure, including or excluding local changes explicitly:

```sh
cargo build --release --bin war-relay --example capacity --example inspection
python3 scripts/mixed-capacity.py \
  --server target/release/war-relay \
  --capacity-client target/release/examples/capacity \
  --sidecar target/release/examples/inspection \
  --users 100000 --seconds 6 --readers 8192 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency --latency-stages \
  --output artifacts/new-build-mixed.json
```

Use a fresh output name. This measures the new executable, not a reproduction of a frozen historical binary. The runner's CPU masks are specific to the recorded Linux host; inspect topology before adapting them. Run comparisons sequentially, with matched settings, fresh processes and no concurrent builds/profiles.

## Read the result correctly

- Treat each user equally. A percentile across users' individual latency statistics is different from a pooled packet percentile. With 24 observations, nearest-rank per-user p99 equals its maximum. The runner reports this explicitly in `latency_sampling`.
- Preserve offered versus completed fanout windows, pre-send generator lag, exact counts, failures and cleanup. Eventual delivery alone does not prove responsiveness. Stage clocks include client scheduling/kernel work; they do not isolate relay-hop latency.
- Add `--pin-client-workers` only as an explicit comparison setting. It records and verifies each client I/O thread's affinity; a process mask alone is not individual pinning. The [short comparison](benchmarks/2026-10-18-mixed-client-pinning/README.md) improves fanout-window maxima at a CPU cost, with overlapping private-user tails. Defaults remain unchanged.
- Logical queue/read-buffer admission budgets are not an RSS cap. Zero routing copied bytes refers to the checked native forwarding path; control encoding, the compatibility bridge and intentional retention compaction have separate ownership/copy behavior.
- Nonzero RSS after all clients leave is not sufficient evidence of a leak. Check owned descriptors, live gauges and exact closes first; the [retirement study](benchmarks/2026-10-18-churn-retention/README.md) distinguishes those from allocator retention.

## Recent alternatives left unselected

| Experiment | Decision and reason |
| --- | --- |
| [Buffered fanout batches](benchmarks/2026-10-18-buffered-fanout/README.md) | Lower CPU/fanout delay did not establish consistently better private-user tails. |
| [512-recipient quantum](benchmarks/2026-10-18-short-fairness-quantum/README.md) | Worsened private-user tails; retain 1,024. |
| [Direct CBOR byte-string parsing](benchmarks/2026-10-18-direct-bytes-screen/README.md) | Faster isolated parsing did not improve measured relay performance. |
| [Socket recv substitution](benchmarks/2026-10-18-socket-recv-screen/README.md) | Lower throughput and higher CPU per delivery in the short screen. |
| [Notification queue handoff](benchmarks/2026-10-18-pending-lock-screen/README.md) | Fewer measured lock collisions did not establish a repeatable application benefit. |

A change earns selection through the required workload and invariants: low and consistent user latency, complete delivery, FIFO, shared payload ownership and bounded resource admission. Neither a microbenchmark win nor a shorter critical section establishes that by itself.
