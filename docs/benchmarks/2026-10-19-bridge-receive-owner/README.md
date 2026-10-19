# Bridge receive ownership: short negative screen

No runtime change selected. Starting from 1186187, two focused checks tested whether the legacy bridge could avoid a CBOR-to-binary payload copy or reduce retained receive ownership. Production services and the existing dirty observer-lock prototype were untouched.

The real native WebSocket receive probe mirrors the bridge's 4 KiB read buffer and zero write-buffer setting. Six self-delivered payloads (0, 1, 24, 1,024, 4,096 and 65,536 bytes) all had shared receive storage. Every envelope had at least eight bytes of prefix space, but none qualified for safe unique-owner mutation. Legacy sender/body conversion was checked for every packet. The six-packet client completed in 0.002 seconds; this is an ownership diagnostic, not a latency benchmark. Client and server exited normally, with connection/resource retirement checked.

The vendored codec splits frames from its reusable read buffer and freezes the resulting view. Prefix space alone therefore does not establish exclusive ownership. No new owned conversion API, unsafe alias mutation, buffer-detachment scheme or fragmentation policy was added. These observations cover this receive configuration and tested frames, not every possible codec ownership state.

A second candidate explicitly converted the packet, dropped its input Bytes, then awaited sending the owned legacy Message. A replacement-allocator diagnostic compared the current bridge and candidate using 256 binary-joined clients, eight verified 1 KiB self echoes each (2,048 total), with four quiescent heap captures. Both passed body/FIFO, counters, retirement and process-exit gates.

| Phase | Control live bytes | Early drop live bytes | Live allocations, both |
| --- | ---: | ---: | ---: |
| Baseline | 96,957 | 96,957 | 174 |
| Joined | 3,873,293 | 3,873,293 | 3,760 |
| Warmed | 4,401,149 | 4,401,149 | 3,764 |
| Retired | 98,045 | 98,045 | 180 |

Baseline-adjusted cumulative allocation counts and bytes were also identical. Absolute cumulative candidate bytes were three higher at every phase, already present at baseline. There is no measured memory benefit to select. Heap profiling uses tcmalloc and is neither ordinary RSS nor a throughput/latency comparison. One pair is sufficient to reject this proposed memory optimization here; it cannot establish general timing equivalence or performance at 100,000 users.

The borrowed converter returns owned output and currently copies binary payloads into one legacy-format allocation. Its Rustdoc now makes this ownership contract explicit. The diagnostic source, controller, receipts and raw heap captures are archived beside this report. Receipt paths refer to the original local artifacts; frozen binary identities are retained in the receipts. Run `python verify.py` to check archived captures and the reported equalities without rerunning load.
