# In-place timer-key update screen — 2026-10-19

**Unselected.** A candidate updated an existing timer heap key in place: sift up for an earlier deadline, sift down for a later one, and return for an unchanged deadline. The selected implementation retains its existing equal-deadline fast path and removes/reinserts changed deadlines. The candidate's small isolated gain for changing deadlines does not establish a relay performance benefit, and its unchanged-deadline path was slower in this screen. No runtime source or running service was changed.

The native worker calls `Timers::set` after driving a surviving connection. The hypothesis was that changing its deadline needlessly repaired the heap twice. This experiment measures that operation directly; it does not establish that retiming dominates actual relay CPU. Existing selected equal-deadline behavior is documented in the earlier [unchanged timer experiment](../2026-10-18-unchanged-timers/README.md).

Each workload prepopulates 100,000 indexed timers, then makes 24 permuted passes (2.4 million calls), changing 0%, 1%, or 100% of deadlines. Changed deadlines move later, as activity commonly extends a timeout. Setup, allocation of the deadline fixture, sorting and exhaustive retirement checks are outside timing. The timed loop includes common fixture indexing and deadline preparation; these are not isolated instruction costs. There are no sockets or application messages. Every run checks the complete final `(deadline, slot)` order, including equal-deadline tie order, and empties the heap.

An initial unpinned sweep also tested 1,000 timers. A second sweep pinned both binaries to the same available CPU and reversed the comparison order. Each case used candidate/control/control/candidate/candidate/control. The following are medians of three measurements per binary from that pinned sweep; lower is better:

| Changed calls | Control ms | Candidate ms | Candidate elapsed change |
| --- | --- | --- | --- |
| 0% | 10.566 | 10.946 | +3.60% |
| 1% | 11.256 | 11.202 | -0.48% |
| 100% | 295.117 | 283.531 | -3.93% |

The 100%-changed candidate gain was about 3.9%; 1%-changed was near neutral. The unchanged case was about 3.6% slower. These narrow results justify leaving the current implementation selected; they do not prove that in-place updates could never help a different workload. No capacity, packet latency, CPU-utilization or zero-copy improvement is claimed. An integrated experiment would require actual changed-deadline attribution first.

Both source variants passed the two existing timer tests in under the runner's 0.01-second reporting resolution. They cover repeated retiming, cancellation, slot reuse, rekeying sparse slots, equal-deadline order, and shrinking. The 36-run initial sweep and 18-run pinned repeat each completed within a few wall-clock seconds. Builds and measurements were sequential, with no profiler or other task workload overlapping the samples.

`provenance.json` records the compiler, exact build flags and source hashes; `pinned.json` also records CPU, executable hashes and commands. `unpinned.json` retains the initial sweep. To reproduce, copy `main.rs` and one `*-timers.rs` into a fresh directory, naming the latter `timers.rs`, compile using the recorded command, then run `./bench 100000 0`, `./bench 100000 1`, or `./bench 100000 100`. Repeat with the other variant in another fresh directory. Compile `main.rs` with `--test` to run the existing tests. `verify.py` audits the archived sources, fixture coverage and complete recorded results without launching a benchmark.
