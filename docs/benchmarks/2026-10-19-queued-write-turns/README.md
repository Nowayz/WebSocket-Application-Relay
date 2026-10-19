# Queued-write turn census: retain the current quantum

Decision: retain the native 256 KiB write quantum. No runtime source, configuration, service executable or default was changed. Two short instrumented mixed workloads show no queued flush reaching this limit, so they provide no basis for adding a smaller scheduling constraint. This does not prove the limit is irrelevant under all workloads.

The current deferred writer holds the socket state lock and flushes queued frames until WouldBlock, queue exhaustion, error, or its byte quantum. The hypothesis was that unusually large queued flush turns might delay other users on the same worker. A copied diagnostic measures that exact operation before changing it.

## Method

Clean committed source at 30d3966 was archived into a fresh directory, excluding the three pre-existing uncommitted relay overlays. The only source patch adds a stack measurement guard inside Socket::flush_locked, after lock acquisition. It counts bytes accepted and complete frames reported by successful write_batch calls. On return it records a fixed-size thread-local byte histogram, elapsed nanoseconds, maxima, maximum bytes and maximum completed frames; one aggregate JSON line is emitted per worker at thread exit. Histogram bucket 0 is zero bytes; bucket i covers [2^(i-1), 2^i-1]. No per-flush allocation or shared atomic is added; diagnostics do add clock reads and local bookkeeping. Logs serialize only on worker exit.

All flush calls across process lifetime are included, including setup, empty queues, measurement and teardown. Immediate sends in enqueue_inner are outside this scope, as are mutex acquisition time, decode/routing, ordinary control writes and event-loop scheduling before entering the function. A zero-byte call may have an empty queue or be unable to write; the diagnostic does not distinguish them. Elapsed values include OS preemption and instrumentation and are not isolated syscall cost or a latency improvement.

Two fresh processes used two traffic seconds each, four individually pinned native workers, eight pinned client workers on separate cores, 2,048 1 KiB fanout readers and four-message fanout windows. One used four private pairs with 64 KiB byte-string payloads; the other used 64 private pairs with 1 KiB payloads. Inspectors/churn/heartbeats/rate limits were disabled. The traffic mixes differ, so this is coverage, not a controlled performance comparison. No builds or tests overlapped traffic. Exact commands, hashes, load settings and logs remain in the receipts. Their generic profile_diagnostic flag is false because it only describes perf/heap runner options; both executables here are explicitly instrumented and excluded from ordinary comparisons.

## Results

| Private workload | All queued flush calls | Zero-byte calls | Largest queued flush | Maximum completed frames in one flush | Calls exhausting 256 KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Four pairs, 64 KiB values | 3,282,193 | 36,927 | 1,041 bytes | 4 | 0 |
| 64 pairs, 1 KiB values | 3,864,337 | 296,209 | 2,082 bytes | 4 | 0 |

These 7,146,530 calls do not exercise the quantum. Successful immediate private sends are not queued flushes; the largest private frame in these workloads is also below 256 KiB. Do not infer that queued frames, slow readers, much larger payloads or burstier traffic can never reach the limit. This evidence argues against tuning the quantum as the next change for these measured mixtures, rather than establishing a universal optimal quantum.

The dominant positive-byte buckets were 16–31 bytes and 1,024–2,047 bytes. Full histograms and diagnostic wall-time statistics are retained in [summary.json](summary.json). These are process-lifetime aggregates, not timed-phase counts, and no meaning is assigned to the small-message bucket beyond observed bytes.

A positive control runs the existing an_exact_write_quantum_retires_empty_queue_storage_before_returning library test against the same diagnostic sources. It deliberately queues exactly 262,144 wire bytes, verifies FIFO and empty queue-storage retirement, and passes. The diagnostic reports max_bytes=262144 and one count in bucket 19. This proves the counter can detect an exhausted quantum; it is not a production traffic benchmark. [Log](positive-control.log).

Both socket fixtures pass full sender/body/FIFO verification, exact message/byte/census counters, zero native routing-copy bytes, normal client/server exits, and final connections/sessions/queued bytes/read reservations/output queue capacity all zero. Instrumented private verified counts were 143,760 and 2,260,016; fanout verified counts were 1,196,032 and 1,507,328. These counts describe diagnostic traffic only. No 100,000-user fairness or ordinary CPU/throughput comparison was performed.

## Evidence and reproduction

[Build receipt](build.json), compressed original socket source, diagnostic-only patch, raw [large-value](large.json) and [small-value](small.json) receipts, summary, and positive control log are archived here. The frozen diagnostic executable is artifacts/candidates/write-turn-20261019-diagnostic/war-relay. The existing client is artifacts/candidates/client-read-window-20261019-control/inspection with source attribution explicitly supplied from its clean snapshot.

Run `python docs/benchmarks/2026-10-19-queued-write-turns/verify.py` for an offline audit. To reproduce traffic, select a fresh output path and use the full commands in each receipt; CPU masks describe this recorded host. Reconstruct the diagnostic by applying diagnostic.patch to src/server/socket.rs in a clean source snapshot at the recorded base commit. Build serially with scripts/build-snapshot.py. For the positive control:

```sh
cargo test --locked --release \
  --manifest-path artifacts/write-turn-20261019-source/Cargo.toml \
  --target-dir artifacts/capacity-client-build --lib \
  an_exact_write_quantum_retires_empty_queue_storage_before_returning -- --nocapture
```

Keep builds, tests and instrumented diagnostics separate from ordinary timed comparisons. The diagnostic remains unselected; there is no added production telemetry, lock, buffer or scheduling policy.
