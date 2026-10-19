# Preserve existing latency timelines

The timeline plotter previously overwrote existing PNG, PDF and JSON outputs. An isolated run of the committed prior script replaced a sentinel JSON file successfully, proving the behavior without touching historical evidence.

The selected script checks all three derived output paths immediately after argument parsing, before reading inputs or importing Matplotlib. Existing paths and dangling symlinks produce exit status 2 with the conflicting path. Fresh rendering behavior remains the same. This is a sequential preflight check, not an atomic cross-process output reservation; keep runs sequential and choose distinct stems.

`python scripts/test-timeline-output.py` exercises six collision cases: each output suffix as a regular file and a dangling symlink, including replacement of an existing suffix in the requested stem. It runs Python without optional site packages and supplies a missing input to prove that collision detection precedes imports and input reads. All six subcases failed against the prior script and passed after the fix in 0.177 seconds. Logs are archived here.

A fresh render used the existing [two-second saved cohort data](../2026-10-18-short-fairness-quantum/raw/short-fairness-sampling-check.windows.json). The resulting [PNG](timeline-preservation-20261019.png), [PDF](timeline-preservation-20261019.pdf) and [summary](timeline-preservation-20261019.json) are included. The PNG was visually inspected: labels, legend, curves and footer were legible. Repeating the command returned status 2 and preserved all three hashes, recorded in [receipt.json](receipt.json). No relay traffic was rerun and this makes no new latency-performance claim.

```sh
python scripts/plot-latency-timeline.py \
  --case 'Saved 100k short run=docs/benchmarks/2026-10-18-short-fairness-quantum/raw/short-fairness-sampling-check.windows.json' \
  --output artifacts/timeline-preservation-20261019 \
  --title 'Timeline output preservation check'
```

Use a fresh output stem to repeat rendering. Runtime source, running services and existing uncommitted relay changes were preserved.
