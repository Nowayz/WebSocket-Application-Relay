# Buffered read before timeout setup — 2026-10-18

Decision: unselected. Keep the existing inspection-client receive helper. The proposed fast path attempts socket.next().now_or_never() before constructing timeout state. It captures an absolute five-second deadline first, using timeout_at with that same deadline for a pending read. It adds no body copy and changes no server code.

The preceding per-process profile made timeout/receive polling a plausible target. It did not prove all timeout self cycles were timer overhead: underlying decoder/socket work may be inlined. This experiment tests one narrowly scoped technique rather than assuming those cycles can be removed.

| Order | Client | Fanout million deliveries/s | Client CPU-s/million | Server CPU-s/million | Fanout maximum ms |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | Control | 1.735 | 2.424 | 1.059 | 7.967 |
| 2 | Candidate | 1.741 | 2.573 | 1.109 | 9.340 |
| 3 | Candidate | 1.854 | 2.436 | 1.078 | 7.461 |
| 4 | Control | 1.689 | 2.409 | 1.105 | 16.613 |

Candidate throughput is higher in these runs, but client CPU per checked delivery does not improve: both candidates exceed both controls. Maxima vary, including a returning-control excursion. This is not a repeatable joint CPU/latency win, and a preliminary poll adds work to pending receives. No claim is made that it would regress every workload, or that timeout work is unimportant. The technique does not yet justify selection.

Each fresh process measures two seconds of pure fanout: 2,048 readers, four messages/window, four pinned native server workers and eight pinned client workers on disjoint CPU masks. Setup and payload/FIFO-checked retirement are additional. Both clients are clean builds of the same source base, except the isolated candidate patch. The native server stays frozen at the latest selected current-100k binary. No compilation, profiler or other benchmark overlaps traffic.

All four fixtures pass payload/FIFO/counter/resource checks with normal process exits. This screen does not establish partial-frame cancellation safety of the extra poll: no new targeted pending/fragmented/timeout tests were justified after the negative selection result. Any revived candidate must pass those checks before adoption. No 100,000-user or end-to-end performance conclusion follows from this small screen.

Full receipts and build logs are retained. builds.json pins both binaries and their true main source hashes. Both receipt source contexts refer to the candidate snapshot; as those receipts state, source context alone is not binary/source association. The explicit control build record and clean Git base resolve that distinction. The patch reconstructs the candidate main source; pre-existing root overlays were excluded. Run python verify.py for offline verification. Root client/native sources, convenience binaries, manual services and protected user edits are unchanged.
