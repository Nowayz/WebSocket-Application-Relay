# Admission retry diagnostic — 2026-10-18

Decision: retain exact admission accounting without adding credit batching. These short probes show relatively few CAS retries; they do not measure the cache-coherence cost of successful atomic operations or rule out admission as a bottleneck at higher concurrency.

Four sequential release fixtures used four pinned server workers, eight pinned client workers, 2,048 fanout recipients and four messages per window. Mixed traffic added 64 private pairs. Order: mixed, fanout, fanout, mixed. Each measured traffic interval was two seconds; setup and FIFO-checked batched retirement were additional. All four fixtures passed exact counters, payload/FIFO checks, normal process exits and final resource gates. No 100,000-user claim is made from these probes.

| Traffic | Reservations | Extra global CAS attempts | Extra attempts per reservation |
| --- | ---: | ---: | ---: |
| Mixed 1 | 4,093,156 | 91,056 | 0.02225 |
| Mixed 2 | 4,006,344 | 84,737 | 0.02115 |
| Fanout 1 | 3,737,376 | 34,111 | 0.00913 |
| Fanout 2 | 3,466,908 | 29,670 | 0.00856 |

Per-peer retries and untracked reservations were zero throughout every measured interval. Counts are closure evaluations minus one per `Peer::reserve` fetch-update, including possible weak-CAS retries. The table is extra attempts divided by calls, not the percentage of calls that retried. Membership snapshot admission outside this method is not counted.

The isolated diagnostic adds cache-aligned per-worker counters, written by each of the four uniquely named native workers using relaxed loads/stores. It avoids a shared diagnostic increment in the measured path. Ready-to-measured deltas exclude setup and retirement. This instrumentation changes layout and instructions, so its CPU and latency results are not optimization comparisons. The probe is not production telemetry and assumes one relay with exactly these four workers.

`raw/diagnostic.patch` applies to the clean commit in `builds.json`; pre-existing workspace overlays were excluded. The binary and modified sources are pinned by SHA-256. Full fixture commands, environment, client identity and assertions are in the compressed receipts. Recompute the table and verify archived receipts with `python verify.py` from this directory or any working directory using its absolute path.

No runtime source or running manual service was changed. Exact global caps, per-peer caps, borrowed payload ownership, FIFO and refunds remain unchanged. A low retry count does not prove cheap successful atomics; further redesign needs separate attribution and a repeatable whole-relay win.
