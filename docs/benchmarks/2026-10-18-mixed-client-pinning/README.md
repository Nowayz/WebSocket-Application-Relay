# Verified client pinning for the mixed-load runner

Code commit `cade20f` adds optional `--pin-client-workers` to `scripts/mixed-capacity.py`, using the existing shared affinity helper. Default placement and native runtime behavior are unchanged. No new affinity implementation, library dependency, server tuning knob or automatic recommendation is introduced.

The mixed runner already pins relay workers but previously gave clients only CPU masks. Individual Tokio workers could migrate within those masks. The new option pins each client's eight known I/O workers to a distinct allowed CPU, records PID/TID/name/original mask/assigned CPU/method/client group, and verifies actual affinity before, during and after the measured traffic phase. It fails on unexpected thread count or changed affinity. It does not pin individual async tasks, main threads or management threads. The final runner also hashes the imported shared helper in its receipt.

## Four six-second trials

Fresh processes run mask/pinned/pinned/mask with the identical native binary, clients, worker counts, CPU masks, payloads and workload. Each has 100,000 private users in 16-user rooms at four 1 KiB probes/user/second, 8,192 fanout recipients, four messages/window at 16 Hz, eight churn workers at 4 Hz and two inspectors. All four complete 96/96 offered windows. Setup and exhaustive checked Close/departure cleanup are outside the six-second traffic phase.

Server workers use physical cores 0–7. Capacity uses logical CPUs 8–11,24–27 (four physical cores with SMT), and sidecar uses 12–15,28–31 (four other physical cores with SMT). No client shares a physical core with the server. Pinning assigns the same eight allowed logical CPUs individually; it does not give the generators more physical cores. Stage clocks are enabled equally. The client never falls behind by its full 250 ms message period; that check is not an arbitrary latency SLO.

| Trial | Client placement | Private population p99 of worst, ms | Worst private user, ms | Worst fanout window, ms | Capacity CPU-s | Sidecar CPU-s | Server CPU-s |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | Mask only | 6.523 | 9.438 | 75.825 | 22.77 | 18.65 | 20.31 |
| 2 | Individually pinned | 11.240 | 15.323 | 62.363 | 24.05 | 19.57 | 21.01 |
| 3 | Individually pinned | 9.791 | 11.688 | 60.065 | 24.17 | 19.66 | 21.17 |
| 4 | Mask only | 11.933 | 14.485 | 79.392 | 23.99 | 19.49 | 20.93 |

Pinned fanout maxima are lower in both repeats, at a small CPU cost. Mean capacity CPU rises about 3.1%, sidecar CPU about 2.9%, and server CPU about 2.3%. These are observed two-repeat means, not population estimates or universal tuning effects. Private-user tail ranges overlap; pinning does not provide a blanket latency improvement or eliminate measurement variability. Keep the setting optional and report it explicitly in future comparisons. Changing client placement is a benchmark configuration change, not a native relay speedup.

Each private user contributes 24 observations. Nearest-rank p99 consequently equals its maximum; the table ranks users' finite observed maxima. Fanout latency begins at window start and includes client windowing/verification. Latency and generator CPU include client work, and stage clocks cannot isolate server-only delays. Do not pool these rows with prior unpinned studies or silently replace their configuration. These short tests do not establish long-term per-user p99, hard deadlines, WAN/TLS latency or a root cause for earlier spikes.

## Validation and provenance

All four cases verify **9.6 million timed private probes and 12,582,912 timed fanout deliveries**, plus separately counted setup/postcheck/departure exchanges. Exact sender/body/FIFO, protocol counters, full fanout matrices, normal closes and descriptor/resource retirement pass. Routing copied bytes and error/pressure/rejection counters remain zero; connection/session/queue/read-buffer/variable gauges return to zero, with only the global channel remaining. Both pinned cases record 16 unique client workers, eight per group. Default cases record an empty affinity array.

A separate final two-second run with 1,600 private users and 512 fanout readers passes all gates, verifies 12,800 private probes, records all 16 pinned workers, and validates the newly added helper hash. It is a runner check, not part of the performance comparison. Python compilation passes. The four performance receipts precede only a help-wording refinement and addition of helper-hash metadata; their exact measured runner is archived separately. The final runner's executable statements otherwise match that measured source. No Rust source changed or native rebuild was required. All three pre-existing user overlays remain byte-identical, and selected native/bridge live executable identities remain unchanged.

## Usage and audit

```sh
python3 scripts/mixed-capacity.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --capacity-client artifacts/candidates/latency-stages-client/capacity \
  --users 100000 --seconds 6 --readers 8192 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency --latency-stages --pin-client-workers \
  --output artifacts/mixed-pinned-reproduction.json
```

Use fresh output names. Omit only `--pin-client-workers` for a matched mask-only comparison. The numeric masks describe this host; inspect topology before adapting the runner. The option changes benchmark child processes only.

Run `python3 docs/benchmarks/2026-10-18-mixed-client-pinning/verify.py` to verify archive hashes, decoded CSV identities/samples/quantiles, worker assignment records, exact counters and final resource gates. JSON, CSV and timeline files are losslessly gzip-compressed; decoded hashes match receipts. The verifier also checks measured versus final runner differences and shared-helper provenance. No network access or server process is required.
