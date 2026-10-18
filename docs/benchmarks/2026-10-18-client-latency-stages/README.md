# Short client latency-stage diagnosis — 2026-10-18

## Result

Diagnostic code commit: `8f52d69` (capacity example and runner only).

The selected native relay remains unchanged. Keep an opt-in diagnostic in the capacity client that separates five elapsed intervals for each private probe. Two eight-second 100,000-user diagnostic runs and an eight-second same-binary run with instrumentation disabled pass all routing, body/FIFO, complete-population, fanout, Close/EOF, zero-copy counter and resource-retirement gates. They cover 9.6 million private probes. A separate four-second no-network timer check supplies 100,000 wake-delay observations. No long soak ran.

Preparation and sequential room sends are small compared with the timer/receive intervals in these runs. This evidence argues against further payload scratch/collection surgery as the next latency fix. It does not prove that the relay or client receive scheduling is responsible for all remaining post-send delay.

| Client-observed stage | p99 range across the 16 one-second instrumented windows |
|---|---:|
| Intended time → room task enters ring | 2.012–2.307 ms |
| Entry → payloads/diagnostic send storage ready | 8.33–13.93 µs |
| Ready → this sender's send call, including earlier room sends | 111.13–152.85 µs |
| Send call → its flush completes | 11.65–14.20 µs |
| Flush completion → recipient application reads packet | 2.602–4.340 ms |

These are separate distributions; their percentiles **cannot be added**. The tool retains a complete stage breakdown for the actual worst intended-latency, worst pre-send-delay and worst post-send probe in each window. For example, the slowest intended probe in the second instrumented run takes **10.302683 ms**:

- Wake delay: 4.185231 ms.
- Preparation: 0.002680 ms.
- Waiting behind earlier room sends/envelope work: 0.091257 ms.
- Its send/flush: 0.006920 ms.
- After send until the application read: 6.016595 ms.

Those five values belong to the same probe and sum exactly to its observed latency. In every measured diagnostic probe the sum is checked before accepting the sample. Runtime preemption is charged to whichever interval it interrupts. The last stage includes client receive scheduling, kernel and relay work, as well as time behind other room sends; it is **not server-only latency**. The clock before `send` is the client call boundary, not a kernel transmit timestamp. Body verification follows the receive timestamp.

## Timer floor and variability

With the same 6,250 room tasks, four ticks/second and eight client workers, but **no sockets or payload work**, the four-second timer check records:

| Wake delay | Time |
|---|---:|
| p50 | 1.040292 ms |
| p95 | 1.747200 ms |
| p99 | 1.925997 ms |
| max | 2.541374 ms |

Pinned Tokio 1.53.3 documents millisecond sleep granularity, and its `TimeSource::deadline_to_tick` rounds deadlines up to a millisecond tick. The included exact installed dependency source supports that mechanism. The no-I/O experiment independently demonstrates a material client timer floor on this machine; it does not establish that all wake delay is timer rounding or all loaded tails have that cause. Do not remove the pre-send delay from intended-latency results or pretend that it was relay processing.

Instrumented client CPU is 31.27 and 31.24 seconds per eight-second traffic phase; instrumentation-disabled CPU is 31.42 seconds. This small sample does not establish an instrumentation cost percentage or performance improvement. Maximum intended latency is 8.809 and 10.303 ms in the diagnostic runs and 22.586 ms in the disabled run. Keep that excursion: the variability is unresolved, and instrumentation can perturb scheduling. These runs diagnose stages rather than compare server policies. Each private user supplies 32 samples, so its nearest-rank p99 equals its maximum; no strong per-user p99 assurance follows from an eight-second screen.

## Kept implementation and verification

`examples/capacity.rs --latency-stages` requires `--user-latency-output`. Per-user stage vectors are allocated for the existing exact sample bound before traffic. Each sample stores five u64 durations (40 bytes); 3.2 million probes require 128 MB of additional sample storage, plus vector metadata and post-measurement aggregation. The extra clocks, temporary send timestamps and storage exist only when this diagnostic is enabled. Existing per-user CSV columns and wire traffic are preserved. Stage quantiles and worst-probe selection are computed after the measured gate, with no timed sorting or file I/O. The JSON adds a labelled stage order, interval definitions and paired worst-probe records. Default/histogram operation does not collect stage samples.

`scripts/mixed-capacity.py --latency-stages` explicitly forwards the option. It verifies all stage cohort counts and worst-probe conservation/identity/maxima and hashes the timeline. The first run precedes that additional runner validation; its exact runner source is retained, and the standalone validator subsequently validates all three cases. Instrumentation-disabled windows must have empty stage data. Four capacity tests pass, including clock partitioning, quantile boundaries and the routing oracle. Strict Clippy for the capacity example, rustfmt and Python compilation pass. CLI misuse without an exact CSV is rejected before connecting.

No server, wire-format, runtime choice, zero-copy ownership, FIFO or queue-limit change is adopted. No manual service is restarted. Original dirty relay/C++ files remain byte-identical. The native runtime is `279c3c1`, frozen executable `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`; the selected bridge remains clean `09f90fb` / `8e8b5c77…`.

## Reproduction and evidence

The archive includes the clean build's library sources, current diagnostic example, runner snapshots, constructor-independent timer example, binary, compiler/build metadata, complete raw CSVs (compressed), JSON receipts and logs. Its dependency pins bind the preceding archives containing full dependency/vendor sources and the frozen relay/inspection executables. These are clean-source builds; the user's three dirty server/C++ files are excluded.

Run `python3 docs/benchmarks/2026-10-18-client-latency-stages/verify.py` to check hashes, source/binary pins, complete sample populations, all per-user quantiles and correlated worst-stage probes. Measured phases never run concurrently. The timer example is built after the disabled run's measured events, while that run is waiting for sidecar retirement; the observed log/build times are retained. It runs only after the final network case exits. No build/profile overlaps a measured network phase.

To repeat on this machine, build the saved capacity example against the saved clean library/dependencies and use the repository runner:

```sh
python3 scripts/mixed-capacity.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --capacity-client /absolute/path/to/built/capacity \
  --sidecar artifacts/candidates/short-fanout-client-v2/inspection \
  --seconds 8 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency --latency-stages \
  --output artifacts/new-latency-stage-run.json
```

Verify CPU topology before adapting the runner's host-specific sets. Relay workers use cores 0–7; capacity uses 8–11,24–27; sidecar uses 12–15,28–31, with no shared physical core between those groups. The traffic remains 1 KiB, 100,000 private users at 4 Hz, 8,192 fanout recipients with four frames/window at 16 Hz, eight churn workers at 4 Hz, and two inspectors.

The next server-performance experiment should first distinguish post-send client receive scheduling from relay/kernel delay. The present evidence supports keeping that question explicit rather than treating the entire client-observed interval as relay latency.
