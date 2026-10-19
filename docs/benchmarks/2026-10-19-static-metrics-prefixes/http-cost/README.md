# Fixed-request HTTP metrics cost, 2026-10-19

Follow-up to the [static-prefix renderer comparison](../README.md), using its frozen control and selected native executables. No runtime source changed for this follow-up.

Four fresh processes in control/candidate/candidate/control order each served 50,000 timed requests after 32 warmup requests. Each trial used one persistent HTTP/1.1 connection, sequential authenticated GET /metrics, one native worker and TOKIO_WORKER_THREADS=1. Server CPU affinity was CPU 0; the Python client used CPU 4. No data-plane clients or traffic were present. No build, test or profiler ran concurrently with timing.

Every response checked status 200, exact Prometheus content type and the entire 7,557-byte body against [the canonical ready exposition](../http-ready.prom). CPU is process user plus system time from /proc/PID/stat with 100 ticks/second. Latency includes client HTTP processing, full body verification, kernel and scheduling costs; it does not isolate server latency. Percentiles use nearest rank.

| Trial | Wall seconds | Server CPU seconds | Client CPU seconds | Median ms | p99 ms | Maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| control-1 | 1.5817 | 0.26 | 1.3991 | 0.030799 | 0.039439 | 0.678531 |
| candidate-1 | 1.5525 | 0.18 | 1.4067 | 0.030340 | 0.043459 | 1.839230 |
| candidate-2 | 1.5408 | 0.18 | 1.4017 | 0.030089 | 0.043489 | 0.677131 |
| control-2 | 1.5678 | 0.24 | 1.3972 | 0.030659 | 0.036939 | 1.187107 |

Server CPU for fixed work averaged 0.25 seconds in the control and 0.18 seconds in the candidate, a 28% reduction (individual control comparisons give 25–31%). CPU quantization limits precision. Wall time averaged 1.5748 versus 1.5466 seconds, a small difference. Both candidate p99 values exceeded both controls; maxima were mixed. This establishes no HTTP latency improvement. The measured client used about 1.4 CPU seconds per trial, which limits interpretation of end-to-end timing; these receipts do not establish the cause of the higher tail values.

All 200,000 timed responses passed verification in 6.2427 total timed seconds. All children exited normally with status zero. Each timed phase had an eight-second watchdog and two-second socket timeout; startup had a three-second deadline and cleanup a three-second graceful wait followed by a bounded kill fallback. No fallback kill was needed. This narrow management-path test makes no 100,000-user fairness, total-machine CPU, or whole-relay throughput claim. Existing manually running services were preserved.

The [driver](driver.py) is the exact executed script; run from the repository root only after preserving/removing its existing artifacts output files, which it intentionally refuses to overwrite. Its executable paths and the per-trial commands and binary hashes are recorded in [receipt.json](receipt.json). Build/source provenance remains in the parent report. [context.json](context.json) records CPU clock resolution, profiler environment absence and total timed duration. Child logs are included. The receipt also retains p95 values.

Run `python docs/benchmarks/2026-10-19-static-metrics-prefixes/http-cost/verify.py` to check the archived evidence without rerunning the workload.
