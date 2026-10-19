# Join partial native startup before returning its error

Native startup previously used `?` directly on OS thread creation. If a later thread could not start, dropping the stop guard signaled earlier workers, but dropping their Tokio join-task set abandoned the joins. The embedding `serve` call could return its error while those OS workers still owned relay state and listener sockets. Signaling stop is not completed retirement.

The failed-spawn branch now drops the existing stop guard explicitly, then uses the existing join-drain helper before returning the startup error. Cleanup has the same configured shutdown timeout plus one-second allowance as normal shutdown; a cleanup timeout is reported separately. Successful cleanup preserves the original `cannot start native I/O worker` error. No worker-count cap, new runtime state, synchronization primitive, test hook or dependency is added. Packet routing and successful startup behavior are unchanged. Dropping/cancelling an embedding future is a separate path and is not changed by this fix.

## Real OS failure reproduction

An unprivileged Linux child receives an `RLIMIT_NPROC` soft limit based on the current UID's thread census plus 32. This changes only the child's limit; the launcher records identical parent limits before/after. The child fills available slots with blocked diagnostic threads, then releases exactly two slots before requesting eight native workers. Both examples are pinned to one allowed CPU to make the immediate-return observation consistent. The remaining blocked diagnostic threads neither spin nor serve traffic and are joined before child exit. The probe uses the public embedding API, with no production fault injection.

The child receives the expected OS thread-start error after partial startup. It immediately records the caller's relay strong-reference count and attempts to bind the same listener address. It then releases its diagnostic blockers, verifies eventual worker retirement and exits normally. No traffic performance is measured.

| Observation immediately after `serve` returns | Control | Fixed |
| --- | --- | --- |
| Relay strong references | 3 (caller plus two workers) | 1 (caller only) |
| Listener address can be rebound | No | Yes |
| Original thread-start error retained | Yes | Yes |

The initial pair and subsequent control/candidate/candidate/control repeats all show these exact results. Every child fills 31 diagnostic thread slots, leaves two available for native startup, exits zero and preserves the parent's resource limits. Control workers eventually retire; the defect is that retirement is not complete when the API returns. The fixed API completes the joins within its cleanup deadline before return. The examples run in milliseconds; their elapsed values are diagnostic durations, not a speedup estimate.

## Ordinary verification

All **110 Rust library tests pass in 0.58 seconds**, including existing join-drain/error/panic and cancellation coverage. Clippy passes with warnings denied. The supported JavaScript checker passes all **61 protocol/legacy/idle cases in 1.127 seconds** against freshly built debug fixtures and requires all six resource gauges to return to zero. Compilation/install time is additional.

The separately frozen optimized fault examples are `artifacts/candidates/worker-startup-cleanup-{control,candidate}/thread_startup_failure`. Their build manifests differ only in `src/server.rs`. This is a source and embedding-lifetime fix; no new release service is promoted and existing manual services/convenience release binaries remain unchanged. The three pre-existing dirty source overlays are preserved. The Linux fault probe requires an unprivileged user whose process limit is enforced and enough room to calibrate at least two slots; it is not a portable CI hook or a system-wide limit change.

Raw probe receipts, source, build manifests and verification logs are retained here. `thread_startup_failure.rs` is the diagnostic example source; `fault-probe.py` is its subprocess launcher. The launcher expects binary/output arguments, creates fresh receipts, and limits only its child. Run `python verify.py` to audit the archived observations and source isolation.
