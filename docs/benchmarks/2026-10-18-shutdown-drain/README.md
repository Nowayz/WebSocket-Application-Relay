# Join remaining service workers after failure — 2026-10-18

Selected reliability fix: cleanup retains the first service failure and continues joining all remaining service tasks, under the existing `shutdown_timeout_secs + 1` outer deadline. This changes only service teardown. Packet routing, locks, queues, payload ownership and scheduling are unchanged; no throughput improvement is claimed.

Previously the cleanup loop used `worker.context("service worker panicked")??`. A task error or panic returned immediately, dropping the remaining JoinSet futures instead of observing their completion. Their native worker stop signals were already published and their own shutdown deadlines remained active, but `serve` could return before the remaining native-thread joins finished. A later cleanup failure could also replace the original failure that triggered shutdown.

The selected helper consumes every task result until the set is empty, retaining the triggering error or the first error encountered during cleanup. The existing timeout still interrupts a worker that does not finish. Deadline expiry remains an error and can still end cleanup before all joins complete; the change does not promise indefinite waiting or change cancellation of a dropped embedding future.

Two controlled regressions fail immediately with the extracted original cleanup loop, before changing its behavior:

- An already-failed task followed by a gated live task must leave cleanup pending until the live task is released. The original loop returns early.
- An error followed by a task that never finishes must reach the outer timeout. The original loop bypasses the wait and returns the earlier error immediately.

After the fix, the first check covers both a task error and a task panic, followed by a second failure. It verifies that the remaining task retires, the set is empty and the first failure survives. A third test preserves the error that originally triggered shutdown. Gating uses oneshot channels; only the deadline check uses a one-millisecond timer. These inject controlled JoinSet failures into the production drain helper; they do not inject a real native I/O thread crash.

Validation:

- Before: the two targeted regressions fail in 0.00 seconds.
- Selected clean snapshot: all 86 library tests pass in 0.54 seconds.
- Working tree including the preserved user overlays: all 89 library tests pass in 0.52 seconds, including existing real-server management/shutdown and queued-write tests.
- Strict all-target Clippy, formatting and the clean release native build pass.
- A disposable release process with two native workers joins four peers, accepts an incomplete HTTP upgrade, then receives SIGTERM. All four peers receive Close `1001`, all five transports reach EOF, and the process exits zero. Observed shutdown is about 1.2 ms in this one local smoke check, with a configured one-second deadline. This is normal-shutdown acceptance, not a shutdown performance benchmark or native-worker failure injection.

The initial binary fixtures incorrectly expected process shutdown to wait for Close acknowledgements. The existing shutdown path deliberately flushes `1001` and retires TCP without waiting; sending a late acknowledgement can cause a TCP reset after the Close. Those two failed fixture receipts and sources are retained. The final probe sends no acknowledgement, verifies the actual flush-and-retire policy and passes. No Close behavior was changed to accommodate the test. The root README now describes that policy explicitly.

`build.json` pins the frozen executable, base commit, exact selected native sources and excluded overlays. `raw/selected.patch.gz` applies to that clean base; build/test logs, the failed and corrected binary probes, raw-client dependencies and all receipts are retained. Run `python docs/benchmarks/2026-10-18-shutdown-drain/verify.py` for an offline checksum, source identity and receipt audit.

No manual service or convenience binary was replaced. The three pre-existing dirty source files remain byte-for-byte unchanged.
