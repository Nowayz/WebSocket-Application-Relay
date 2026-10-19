# Stability receipt ownership, 2026-10-18

Source base: `bbacfda`. This is a runner reliability fix, with no relay runtime or performance claim. The measured `run_case` implementation is byte-identical before and after.

The original runner overwrote a controlled sentinel receipt during a successful one-second run (`overwrite-proof.json`). The pre-fix regression log has three failures and one error. The selected runner checks existing paths early, exclusively creates the file after metadata collection but before launch, and keeps the same descriptor for updates. A collision during metadata collection also fails before launch. Workload exceptions retain metadata, earlier completed cases and the original error representation. This is not an atomic replacement or power-loss durability guarantee.

Four focused regression methods pass in 0.278 seconds: existing regular/dangling paths, creation during metadata collection, startup failure with child reaped, and a mocked later failure retaining the first completed case. Existing four live-pressure predicate tests also pass. The mocked case does not represent a real second relay workload. Repeating the original successful command against its existing result exits 2 and preserves its SHA-256 (`preservation-proof.json`).

Live selected checks used the same frozen native/stress binaries recorded in each JSON, totaling six configured traffic seconds. All cases checked payload/FIFO, counters, retirement and clean relay exit without forced termination.

| Mode | Actual traffic seconds | Verified deliveries | Observed maximum ms |
| --- | ---: | ---: | ---: |
| traffic | 1.000 | 1,476,608 | 0.889145 |
| churn | 1.000 | 114,753 | 0.891575 |
| slow | 2.000 | 86,461 | 0.607374 |
| fanout | 2.000 | 927,780 | 0.831407 |

These small loopback screens do not establish a 100,000-user latency SLA or compare Rust with C++. The fanout gate observed exactly two slow consumers and eight healthy sessions/connections before teardown, with seven readers plus the publisher. Final connections, sessions, queued bytes, read-buffer reservations and output-queue capacity all returned to zero. Mixed-size routing intentionally compacted small values retained from large slabs; it is not universally copy-free. Timing numbers are observations, not an optimization comparison.

Reproduce focused checks from repository root:

```sh
python3 scripts/test-stability-output.py
python3 scripts/test-stability-gate.py
```

Live commands are preserved in the shell logs' corresponding JSON case commands; original/preservation proofs retain the complete wrapper invocation. Use a fresh output path. Frozen executable hashes and working-tree provenance are in each receipt; dirty source metadata does not imply those overlays were compiled into the frozen binary. Original sentinel, overwritten successful receipt, original/selected runner source and test source are included. Run `sha256sum -c SHA256SUMS` here to check the evidence bundle.
