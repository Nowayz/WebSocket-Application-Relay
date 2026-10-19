# Release the legacy join owner after encoding, 2026-10-19

Source base `6c0d3a7`. Selected change: encode the CBOR subscription into its existing owned allocation, explicitly drop the original legacy join Bytes view, then send that subscription upstream. The former view is never used again. No additional allocation, data copy, queue, public setting, packet-path branch or protocol change is added. The rest of bridge(), including welcome validation and post-welcome forwarding, is byte-identical. The original candidate build is selected after these checks; its artifact path is retained unchanged for executable-map/symbol reproducibility.

A binary legacy join can be a tiny view into the decoder's 4-KiB read allocation. Holding that view in the long-lived bridge future prevents prefix reclamation when later packets exhaust the remaining receive capacity. The codec then needs another receive allocation and split bookkeeping while the join still pins the old storage. This is bounded retention until disconnect, not an unbounded leak. The independent CBOR subscription already owns the channel bytes, so retaining that original view is unnecessary.

The baseline was measured first. Two sequential server-only gperftools diagnostics replace only the bridge allocator, leaving the native relay/client unchanged. Each has 256 binary-joined clients in distinct rooms, eight changing 1-KiB self-probes/client, all sender/body/FIFO checks, normal Close acknowledgement and TCP EOF. Four quiescent gates capture requested live/cumulative heap: baseline, joined, warmed, retired. No performance timings/RSS from these diagnostic allocator runs enter ordinary comparisons. Every heap row is summed against its header and native executable return addresses are symbolized through the recorded PIE mapping (return address minus one).

| Requested live heap after checked traffic | Before | Selected |
| --- | ---: | ---: |
| Total bytes | 5,459,965 | 4,401,149 |
| Live allocation objects | 4,276 | 3,764 |
| BytesMut::reserve_inner group bytes | 1,048,576 | 0 |
| BytesMut::split_to group bytes | 30,720 | 20,480 |

The reduction is exactly 1,058,816 requested live bytes and 512 objects: 4,136 bytes/two objects per binary-joined client in this workload. The extra 4-KiB receive allocation accounts for 4,096 bytes/client, with 40 bytes/client of split bookkeeping. All other symbol groups' requested live bytes are identical at the warmed gate. Baseline (96,957 bytes), joined (3,873,293 bytes) and retired (98,045 bytes) live totals are identical between variants. This phase-specific result supports the pinned-buffer mechanism rather than a smaller object-layout estimate. Warm-phase cumulative allocation also falls by exactly 1,058,816 bytes/512 objects when subtracting the joined capture. Startup cumulative totals differ by seven bytes; this small startup variation is not credited to the change.

Four fresh default-allocator cases then run sequentially in control/candidate/candidate/control order, with the same 256 clients and 2,048 exact probes each. No build, tests, profiling, symbolization or compression overlaps these ordinary memory cases. Four Tokio bridge workers are requested identically. This is a quiescent memory census, not a CPU/throughput/latency comparison; the runner's elapsed traffic fields remain raw observations and are not presented as speed evidence.

| Run | Joined bridge RSS KiB | Warmed bridge RSS KiB | Warmed anonymous KiB |
| --- | ---: | ---: | ---: |
| 0 control | 9,312 | 10,908 | 5,916 |
| 1 candidate | 9,320 | 9,876 | 4,884 |
| 2 candidate | 9,420 | 9,980 | 4,888 |
| 3 control | 9,364 | 10,952 | 5,912 |

The ordinary warmed RSS ranges separate in these short repeats, about 1 MiB lower across 256 clients. Requested heap bytes exclude allocator bookkeeping/free caches, stacks and kernel socket memory, and are not RSS. Default-allocator retired RSS need not return to baseline because freed allocations can remain in allocator arenas. The profiler's retired requested live totals do match. No per-client saving is extrapolated to all channel modes, packet sizes, allocators or 100,000 users. Text joins currently copy their small name separately, so the measured 4-KiB pinning mechanism specifically describes binary joins.

All six memory cases (two diagnostic, four ordinary) verify exact native input/delivery/retirement counts, zero queue/variable resources and no errors/refusals/slow consumers. Each verifies 2,048 complete changing probes, for 12,288 total. Both diagnostic cases restore bridge FD count 10 to 10; both processes exit zero in every case. The native routing-copy counter is zero; the migration bridge still performs its existing format conversions, so this is not an end-to-end no-copy claim.

The candidate is freshly compiled (fresh:false); complete source/binary hashes and original/selected sources are retained. All 106 library tests pass in 0.52 seconds, Clippy with tests/warnings denied and formatting checks pass. The full 61-test native/legacy/JavaScript suite passes in 1.103 seconds, with both fixture processes exiting normally and final tracked gauges zero. This includes text/binary legacy joins, original relay.js compatibility, welcome policy/validation, and modern client cases. Existing unit tests are reused; no new implementation-mirroring tests are added for the explicit drop.

No multi-hour or 100,000-user campaign is run. Ordinary memory cases take fractions of a second; setup, profiler captures, compilation and conformance testing add wall time. All existing manual services, convenience binaries and three pre-existing source overlays remain untouched. Only src/legacy_bridge.rs production ownership changes. The verified selected executable remains artifacts/candidates/bridge-join-owner-candidate/war-legacy-bridge; that original candidate path is an identity, not a statement that the implementation remains unselected.

The raw eight heap dumps, exact row/group analysis and symbols, two diagnostic receipts, four ordinary receipts/commands, helpers/reference client, build identities and final compatibility receipt are archived. Run python3 verify.py here for a portable offline audit. Binaries remain at their original paths and are SHA-pinned; they are needed to regenerate symbols, not to audit stored rows. Artifact drivers assume repository-root cwd and their original artifacts/scripts layout. Copy the archived helpers into those locations with fresh output/profile prefixes to reproduce the checks.
