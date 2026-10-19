# Skip discarded poll-timeout calculations — 2026-10-18

Decision: unselected. Choosing the zero poll timeout before reading the clock when the ready queue is nonempty removes discarded work, but four short release fixtures did not demonstrate a repeatable whole-relay benefit.

The isolated candidate only reorders the existing final poll-timeout calculation. The ready batch remains bounded, timer processing and connection driving retain their existing clocks, and both versions poll with zero timeout when ready work remains. No timestamps, queue ownership, payload storage, admission accounting or FIFO behavior were changed. The candidate was not integrated into the workspace runtime.

| Order | Variant | Server CPU-seconds per million checked deliveries |
| --- | --- | ---: |
| 1 | Control | 1.2501 |
| 2 | Candidate | 1.2383 |
| 3 | Candidate | 1.2654 |
| 4 | Control | 1.2673 |

Each fixture measured two seconds of unpaced mixed traffic: 64 private pairs, 2,048 fanout readers, four messages per fanout window, four pinned server workers and eight pinned client workers. Setup and retirement were additional. All passed payload/FIFO, exact counters and final resource gates with normal process exits. Workload mix varies with scheduling; normalizing by checked deliveries is descriptive, not a fixed-work causal comparison. CPU costs overlap and latency/throughput do not show a consistent joint improvement. No large-scale or SLA claim follows.

Control is the frozen selected single-entry writer. Candidate is a clean archive of the base commit in summary.json plus raw/candidate.patch; the three pre-existing workspace overlays were excluded. Receipt hashes identify both binaries. Full commands and client identity are retained in each compressed receipt. No compilation or profiling overlapped these four timed fixtures. The build log and reconstructable patch are archived.

Run `python verify.py` to verify portable receipts and recompute normalized CPU costs. Manual services and protected user edits were preserved.
