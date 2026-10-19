# Exact storage for public encoding helpers — 2026-10-18

Selected: size `protocol::text`, `protocol::envelope` and `protocol::reply` for their actual definite CBOR headers, following the existing byte-string helper. Their wire format, owned return values and shared-clone payload storage are unchanged. The native forwarding path already uses borrowed Frames; this is a public-helper construction improvement, not a native relay throughput claim.

The broader candidate also sized integer storage exactly. It regressed integer construction, so `protocol::uint` retains its existing nine-byte reservation. No extra runtime mode or configuration was added.

Each table entry is the range of two fresh process medians from sequential control/candidate/candidate/control runs. Each process measured seven batches of 100,000 constructions per case, pinned to CPU 4. Builds and other traffic did not overlap measurement. Clone cases make and drop eight clones sequentially; they are not concurrent consumers or socket fanout.

| Operation | Control, no clones (ns) | Selected, no clones (ns) | Control, eight clones (ns) | Selected, eight clones (ns) |
| --- | --- | --- | --- | --- |
| 16-byte text | 19.34–19.68 | 14.77–15.14 | 154.70–155.30 | 148.65–149.83 |
| 1 KiB text | 27.12–27.34 | 19.60–19.66 | 157.06–162.04 | 163.40–164.33 |
| Small-ID envelope | 24.79–25.05 | 18.73–18.84 | 159.94–159.94 | 159.74–161.36 |
| Maximum-ID envelope | 18.84–19.66 | 19.38–19.51 | 155.43–155.83 | 155.10–155.48 |
| Reply operation 0 | 24.67–25.18 | 18.55–18.60 | 156.82–159.19 | 159.43–161.45 |
| Reply operation 255 | 24.67–25.17 | 18.70–18.75 | 157.09–159.24 | 158.88–159.71 |

Unshared construction improves roughly 23–28% for cases previously retaining spare capacity. Clone-heavy costs do not uniformly improve: 1 KiB text is a few ns slower, and some reply cases are slightly slower. These tradeoffs are retained explicitly. Maximum-width envelopes already had exact capacity and do not establish a gain. The benchmark does not count allocator calls or establish end-to-end application performance.

Requested capacity for 16-byte text changes 25→17 bytes and for 1 KiB text 1,033→1,027. The sampled small-ID envelope changes 27→19, and replies change 30→21/22 depending on opcode width. Capacity is not allocator usable size or RSS. In pinned Bytes, exact Vec capacity permits deferred shared ownership metadata; first cloning still has a cost. Ownership tests recover the unique mutable buffer with the same body pointer and capacity equal to encoded length.

The initial rejected integer variant regresses unshared integer 1 from 10.36–10.45 ns to 14.39–14.43, and integer 65,536 from 10.82–10.86 to 14.40–14.84. All initial data are preserved. The selected narrower variant restores original integer behavior; its unshared measurements overlap control.

Validation: all 84 working-tree library tests pass (including preserved user changes); all targets check; both clean-snapshot release binaries build. New storage tests cover empty/UTF-8 text, lengths 23/24, 255/256 and 65,535/65,536, every sender header width, opcode 23/24, exact decoding and clone/recovery pointers. The independent socket corpus passes 776 cases and 2,311 exchanges with healthy traffic in 0.831 seconds. No manual service or convenience binary was replaced.

`builds.json` pins frozen executables and selected source. Patches apply to its clean base, excluding pre-existing root overlays. The native snapshot contains the selected protocol source; benchmark source is included. Full raw batches, build/test logs and corpus receipt are retained. Run python verify.py to recompute medians, capacity changes and archive checksums. The rejected wide patch includes the same final test code for reconstructability; tests were added after its benchmark and do not participate in release benchmark compilation.
