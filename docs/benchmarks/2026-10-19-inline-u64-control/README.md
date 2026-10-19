# Inline full-width control integers

Selected: use fourteen inline prefix bytes instead of ten and remove the separate allocated uint64 reply fallback. The maximum command prefix is five bytes and a CBOR uint64 needs nine, so every unsigned control reply now fits inline. Existing width-vector tests verify exact bytes for all integer widths and command byte widths, and now require an empty heap payload for every integer reply. Welcome, routing, variable and boolean semantics are unchanged.

The native frame size is **48 bytes before and after** on this Linux x86-64 build, verified by compiling both layouts and recording `size_of::<Frame>()`. The larger inline prefix consumes alignment space; it does not enlarge frames or their containing output queue entries on this build. This is a target-specific layout observation, not a portable ABI guarantee. Prefix initialization still clears its full inline array. No unsafe code, new state field, dependency, lock, quota or buffering policy is added.

## Focused allocation evidence

Fresh server-only tcmalloc diagnostics warm 64 sequential connections, authenticate a peer, explicitly assign it ID 42, then verify 64 repeated same-ID assignments. They next assign `2^63 + 17` and verify 64 repeated full-width assignments. Every response is checked byte-for-byte by an independent masked WebSocket client, with normal Close/EOF and exact final server counters. Only the server receives the profiler library.

| Probe, 64 operations | Control allocation objects / requested bytes | Candidate allocation objects / requested bytes |
| --- | --- | --- |
| ID 42 | 48 / 53,044 | 48 / 53,044 |
| ID 2^63 + 17 | 112 / 53,620 | 48 / 53,044 |

Boundary admin requests and dump overhead are included in these totals. Symbolized stack deltas identify exactly **64 allocations and 576 bytes** in the old `reply_uint`/`protocol::uint` path, absent in the candidate. That is one nine-byte allocation removed per full-width control reply. Live heap deltas are zero in both probes and both versions. Every raw row sums to its heap header and raw capture hashes are recorded. This does not imply an RSS or latency saving of nine bytes per connected user.

The initial allocation fixture accidentally used its randomly generated welcome ID for the supposed small-ID probe; that was also full-width. The corrected v2 fixtures explicitly assign 42 first. Preliminary captures remain under `artifacts/native-inline-u64-*-heap.json`; only v2 results are used here.

## Short traffic checks

All ordinary traffic runs used freshly frozen control/candidate releases, sequential control/candidate/candidate/control order and two-second measured traffic intervals, plus setup and verified teardown. No builds or profiling overlapped ordinary traffic. Four server workers and eight client workers use separate pinned CPU sets.

The first, unpaced saturation screen used 2,048 fanout readers, four messages/window, 64 private pairs and 1 KiB byte-string values. CPU seconds per million deliveries were 1.285/1.278 for controls and 1.246/1.270 for candidates. One candidate produced more fanout traffic and had a worse private tail (5.988 ms versus control 3.130–3.234 ms). Because traffic mix was unpaced and varied, this screen alone did not resolve the latency question.

A fixed-rate follow-up held 4,096 private users plus 2,048 fanout readers, four messages/window and 16 fanout windows/second, with no inspection, extra private pairs or churn. Exact user samples and fanout samples remained enabled.

| Run | Server CPU seconds | Worst private ms | Worst fanout ms |
| --- | --- | --- | --- |
| Control | 0.56 | 2.939 | 6.133 |
| Candidate | 0.55 | 2.866 | 5.471 |
| Candidate | 0.59 | 2.535 | 5.535 |
| Control | 0.61 | 2.341 | 5.770 |

All four follow-ups met the offered fanout rate and passed correctness and cleanup with all three processes exiting zero. CPU and private latency ranges overlap; these finite checks do not demonstrate an overall relay speedup, an SLA, or 100,000-user performance. The directly supported gain is simpler allocation-free full-width control reply construction without a frame-size increase on this target.

All 106 library tests passed in 0.50 seconds; Clippy passed with warnings denied. The complete 61-case JavaScript socket suite passed against the newly built native candidate and the selected legacy bridge, with resource gauges returning to zero. Existing manual services and unrelated dirty source files were preserved.

The receipts, build manifests, layout/test logs and summary are retained here. Raw heap paths remain referenced in diagnostic receipts. Helpers are `artifacts/native-inline-u64-heap.py`, `artifacts/analyze-control-frame-heap.py`, `artifacts/native-inline-u64-screen.py` and `artifacts/native-inline-u64-paced.py`. Run `python verify.py` for the offline report audit.
