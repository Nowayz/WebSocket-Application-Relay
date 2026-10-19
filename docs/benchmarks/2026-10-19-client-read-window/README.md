# Inspection client 8 KiB read buffer — rejected short screen, 2026-10-19

Decision: retain the existing 4 KiB inspection-client read buffer. An 8 KiB candidate worsens client CPU per verified delivery, throughput and observed fanout maxima in both candidate trials relative to both controls. Native server/client defaults and runtime source are unchanged. No full-population retest is justified by this failed short screen.

The hypothesis was specific: four typical native CBOR fanout packets at 1 KiB values occupy about 4,164 wire bytes, just above 4 KiB. Larger capacity might coalesce reads across a four-message window. This is not an established syscall bottleneck: TCP arrival timing and the decoder matter, and no syscall-count diagnostic is used here. Earlier buffered-read/timeout/window-deadline candidates were already rejected and were not repeated; this candidate changes only read_buffer_size(4096) to read_buffer_size(8192) at inspection client connection setup.

Both clients are independently frozen from clean current source snapshots, with Cargo fresh:false and source/executable hashes. Their input manifests differ only in examples/inspection.rs. The common frozen native server is unchanged across all trials and pinned by each receipt. The candidate passes all nine inspection example tests in 5.00 seconds, including non-primary reader corruption, departure corruption and the controller acknowledgement deadline. Per-message timeouts, full body/sender/FIFO checks and frame limits are unchanged.

Four fresh ordinary processes run sequentially in control/candidate/candidate/control order. Each has two seconds of unpaced traffic, 2,048 fanout readers, four-message windows, 1 KiB byte values, four native workers on CPUs 0–3, and eight individually pinned client workers on CPUs 4–7,20–23. Inspection, private pairs and churn are disabled. Departure batching at 32 only shortens checked teardown outside measurement. Builds/tests/profilers do not overlap timing. Setup/retirement add wall time beyond the eight total traffic seconds.

| Run | Verified fanout deliveries | Client CPU-s / million | Native CPU-s / million | Deliveries/s | Fanout maximum ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 3,612,672 | 2.4691 | 1.1460 | 1,802,138 | 7.448 |
| Candidate 1 | 3,129,344 | 2.6875 | 1.1887 | 1,559,537 | 9.145 |
| Candidate 2 | 3,497,984 | 2.6329 | 1.1635 | 1,745,950 | 8.300 |
| Control 2 | 3,629,056 | 2.5075 | 1.1435 | 1,810,424 | 7.127 |

This is an unpaced throughput screen, so CPU is normalized by verified fanout deliveries; lower absolute CPU with fewer deliveries is not an efficiency win. Sampled ready-gate client RSS is 14,652/14,608 KiB for controls and 22,900/22,840 KiB for candidates, consistent with higher upfront capacity. At the measured gate the candidate RSS is lower (26,512/26,528 versus 28,332/29,368 KiB), but different delivery totals and allocator history prevent a fixed-work or peak-memory conclusion. No causal kernel/cache attribution or universal buffer-size claim is made.

All four trials pass exact payload/FIFO/counter checks, normal process exits and retirement. Final connections, sessions, queued bytes, read reservations, native connection slots and output queue capacity are zero, only the global channel remains, error/pressure/rejection counters remain zero and routing copied bytes remain zero. These finite maxima do not establish long-term percentiles, relay-hop latency or 100,000-user acceptance for the candidate.

Run `python docs/benchmarks/2026-10-19-client-read-window/verify.py` to recompute the comparison and audit source/build/resource boundaries. compare.py retains exact commands and requires fresh output names for repetition. Sources and all receipts are retained. Existing manual services, convenience binaries and the three unrelated dirty runtime files are preserved. The current guide also resolves a stale suggestion to try timeout variants: those experiments were subsequently completed and rejected, as already recorded in its decision table.
