# Fanout send-order audit and timing-origin correction

Selected: correct the comparison plot's “From actual send” title to “From client measurement start,” with explicit JSON metadata describing the source-client clock. Existing metric names remain for compatibility. The current fanout stamp is taken before `watch::send_replace`, which notifies all readers, and before socket sends. Its duration therefore includes publisher-side wakeup work, socket/kernel/relay/client scheduling and payload checks. It is not an isolated server-hop or exact post-send measurement.

The latest 100,000-user private/fanout plots are regenerated and visually inspected with the corrected title. Every case record, CSV hash, percentile, threshold percentage and plotted curve is unchanged. Original PNG/PDF/JSON renderings are retained here; the source benchmark receipts are untouched. Private timestamps are also source-client timestamps rather than server-hop clocks, so the generic caption remains accurate for mixed CSV inputs.

An isolated client scheduling experiment sends the prepared window before notifying its reader tasks, retaining the timestamp before the first write. It preserves intended-send timestamps, every payload/sender/FIFO check, acknowledgement/departure policy and sample count. The optional controller `sent` clock remains immediately after the final write, before notification. Sending first could overlap reader notification with server processing, but might defer application reads; this is a benchmark-client change, not a server optimization.

Decision on that experiment: retain the existing ordering. Four ordinary two-second trials run control/candidate/candidate/control with 64 private pairs, 2,048 fanout readers, four-message windows and 1 KiB byte values. The unchanged native server has four workers on CPUs 0-3; eight client workers use CPUs 4-7,20-23. Traffic is unpaced, with no inspectors or churn. All builds/tests are separate from timing. The same copied value-kind source snapshot is rebuilt for both clients; manifests differ only in `examples/inspection/fanout.rs`.

| Variant | Native CPU-s/million deliveries | Private maximum (ms) | Fanout maximum (ms) | Fanout deliveries/s |
| --- | ---: | ---: | ---: | ---: |
| control | 1.2951 | 7.272 | 14.529 | 756,015 |
| candidate | 1.2812 | 7.556 | 19.635 | 751,779 |
| candidate | 1.2410 | 4.739 | 12.081 | 866,748 |
| control | 1.2950 | 5.645 | 16.433 | 768,422 |

One candidate run is faster and one slower in its observed maxima; the unpaced delivery mix also varies. This does not establish a repeatable benefit or prove wakeup order caused the preceding full-population jitter. The small screen is enough to leave the client unchanged and avoid a larger campaign for this candidate.

All four trials pass payload/FIFO/counter/resource-retirement checks, with clean exits and zero recorded routing copies. All eight candidate inspection-example tests pass in five seconds (including an intentional five-second deadline test); all nine plot semantic tests pass in 0.031 seconds. Existing native/bridge services, convenience executables and protected dirty source files are preserved. No performance or capacity improvement is claimed from relabelling the figures.

Run `python docs/benchmarks/2026-10-18-fanout-send-order/verify.py` to audit source identities, the rejected experiment and unchanged figure statistics. The original 100,000-user report's checksum manifest is refreshed for its corrected prose/renderings; its raw evidence remains unchanged.
