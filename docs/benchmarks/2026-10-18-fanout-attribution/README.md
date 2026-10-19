# Large-fanout delay attribution — 2026-10-18

The previous ordinary latest-source 100,000-user acceptance completed 96/96 offered windows but had recipient maxima up to 64.84 ms. This study decomposes its existing stage records and runs one separate six-second diagnostic with user-cycle sampling of the server and fanout client. It does not select a runtime change or compare profiled latency with ordinary latency.

| Controller interval, ordinary run | Median ms | p99 ms | Maximum ms |
| --- | ---: | ---: | ---: |
| Scheduled time → publisher wake | 1.024 | 3.479 | 3.621 |
| Wake → all four sends complete | 2.125 | 2.491 | 2.509 |
| Last send complete → all recipient acknowledgements collected | 51.713 | 62.420 | 62.736 |
| Acknowledgements collected → controller bookkeeping complete | 0.00036 | 0.00058 | 0.00059 |
| Scheduled time → complete | 54.904 | 65.978 | 66.120 |

There are 96 records; 92 have more than 40 ms after submission. These intervals use the existing common origin. Post-submission includes server routing/output, client reads and decoding/verification, scheduling and acknowledgement collection. It is not isolated server/network time. The reader's send-relative sample origin occurs between publisher wake and completion of sends, so do not equate a stage percentile with a recipient percentile or subtract unrelated percentiles. Summaries use medians and floor-index population p99; raw records remain available.

The diagnostic uses the same source/binaries, 100,000 private users, 8,192 fanout readers, six offered seconds, window four, 16 Hz fanout, 4 Hz private/churn and explicit thread pinning. A disposable copy of the runner changes only the perf PID set to server+sidecar and records that scope; the private capacity client is excluded. It passes all population/counter/retirement gates and all 96 offered windows, with all three process exits zero. Setup/retirement add wall time. Native sources and manual services are unchanged.

Per-process reports use perf report --pid PID --percentage relative --no-children. Server self samples include drive_batched 17.59%, receive_bounded 16.20%, Worker::run 8.56%, enqueue_fanout 5.18%, notify 5.03% and schedule 4.99%. The sidecar has Timeout::poll 20.76%, reader closure 6.87%, poll_ready 5.86%, shared_v_drop 5.22%, TimerEntry::drop 3.72%, task wake 3.67% and Sleep::poll 3.37%. Both reports show zero lost samples. Percentages are per filtered process, not shares of wall time. Sampling is user-space cycles, not a kernel or blocked-time profile; unresolved syscall trampoline samples appear. Timeout::poll may include inlined socket/decoder work, so its whole percentage is not proven removable timer overhead.

The reader helper wraps every socket.next() in a fresh five-second timeout. In this fixture nearly all sidecar application receives are fanout frames. A candidate next experiment is to consume already-ready data before constructing timeout state, while retaining the original absolute deadline for pending reads and testing cancellation safety with partial frames. This evidence justifies that experiment; it does not prove its benefit or attribute the complete 50–60 ms interval to client timers. Avoid redesigning server batching from this profile alone.

The stage analysis uses the ordinary receipt; diagnostic timings are excluded from performance conclusions. Compressed ordinary/diagnostic receipts, exact runner, helper excerpt, relative reports and recomputed summary are retained. The large raw perf file remains at the absolute local path and SHA-256 in summary.json; it is not needed for offline verification. Run python verify.py to verify checksums, stage math, source/binary matching, completed offers and final resource gates. Existing full per-user CSVs remain in the preceding current-100k-acceptance archive.
