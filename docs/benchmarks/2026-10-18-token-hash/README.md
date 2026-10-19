# Internal token hash screen — 2026-10-18

Decision: unselected. An integer hasher for the worker's readiness-token map did not deliver a repeatable whole-relay benefit. The ordinary randomized map remains selected.

The isolated candidate changes only HashMap<Token, usize> to BuildHasherDefault<TokenHasher>. TokenHasher multiplies server-generated usize token values by an odd 64-bit constant; its generic byte fallback rotates/xors bytes. Client-controlled channel names, user IDs and other relay maps are untouched. Readiness tokens still never repeat; no slot encoding, queue, lock, wakeup, timing, payload ownership or admission rule changes. The map's initialization and empty-arena reset use Default for the specialized builder.

| Workload | Control CPU-s / million deliveries | Candidate CPU-s / million deliveries |
| --- | --- | --- |
| Fanout | 1.1913, 1.1304 | 1.0848, 1.1310 |
| Mixed | 1.2821, 1.2424 | 1.3986, 1.2659 |

Each workload ran sequential control/candidate/candidate/control fixtures. Each measured traffic interval was two seconds, plus setup and retirement, using 2,048 fanout readers, four messages per window, four pinned server workers and eight pinned client workers. Mixed adds 64 private pairs. Initial fanout results looked favorable, but the returning control matched the slower candidate and mixed CPU costs did not support adoption. Mixed candidate private maxima were 5.88/8.45 ms versus control 3.14/3.60 ms; fanout maxima were 18.79/22.51 ms versus control 16.65/13.40 ms. These finite maxima are diagnostic observations, not latency SLAs. Unpaced workload mix varies, so normalization is descriptive rather than a fixed-work comparison.

All eight fixtures passed payload/FIFO, exact counters and resource-retirement checks, with normal client/server exits. No build, profiler or other benchmark overlapped timed traffic. No large-population claim follows from these short screens. No runtime source or manual service was changed, and the three pre-existing dirty source overlays were preserved.

The patch reconstructs the candidate from the clean base in summary.json, excluding those overlays. The successful build log and all receipts are retained. Binary hashes, full commands and client identity are in the receipts. Run python verify.py to recompute the normalized CPU figures and verify checksums/retirement gates. The purpose of retaining this negative result is to avoid adding specialized hashing based only on its sampled profile cost.
