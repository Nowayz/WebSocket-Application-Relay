# Notification screen and verified worker placement — 2026-10-18

## Decision

Retain the native socket's existing atomic-swap wake coalescing. A small mutex-serialized load/store prototype passes correctness tests, but eight short network comparisons do not establish a repeatable CPU-cost improvement. No Rust relay change or deployment is adopted.

Keep code `1745b2a`: the inspection/fanout runner now supports the same explicit worker-placement controls as the capacity runner. The implementation shares the existing placement helper, preserving its old capacity import as an alias. Historical defaults remain unchanged.

## Profile and isolated candidate

A four-second diagnostic profile of selected executable `3db9ebeb…` captures **6,159 user-cycle samples, zero lost**. About 10.03% of sampled user cycles are attributed to the publisher's `Socket::notify`; annotation places much of its local weight immediately after the atomic `xchg`. This locates a candidate, not a proof that replacing that instruction makes the whole relay faster. Kernel cycles are excluded, and diagnostic timing is not included in ordinary comparisons. Raw 49.461 MB perf data remains local; its size/hash is recorded while text reports and the complete profile receipt are archived.

Every notification producer already holds the socket's state mutex. The prototype requires a borrowed `MutexGuard<State>` argument and replaces `notified.swap(true)` with a relaxed load/conditional store. The owner still resets before driving under that state lock. If a producer sees an old true flag while the owner is clearing, its locked publication is consumed by that drive; if it follows an owner's completed drive, mutex synchronization makes the reset visible. A concurrent false-load/reset/store leaves a queued wake. The flag is not the payload-publication mechanism; the existing locks are. All three producer call sites are audited.

The clean candidate passes **80 library tests**, including two added tests for coalescing and deterministic owner-reset-before/after-publication interleavings. Existing full-body pointer-sharing, FIFO, short-write, reservation, policy and retirement tests also pass. The candidate requires additional reasoning compared with the existing swap; without a demonstrated application benefit it is not worth adopting. These tests/source remain an isolated experiment, not changes to the selected library.

Control and candidate are built sequentially from clean base `077b948`, in the same project directory, with the same compiler/dependencies/release settings. Only `src/server/socket.rs` changes in the release comparison; original user-modified files are excluded. Candidate test code is appended after those release builds; both the actual release source and tested source are retained separately. No build/profile overlaps the ordinary measured phases.

## Eight four-second fanout phases

Every case has 4,096 room recipients, 1 KiB values, four-frame windows, four native workers on physical CPUs 0–3 and eight client workers allowed physical CPUs 4–11. Each window's full audience/body/FIFO is verified. No private pairs, churn, inspectors or unrelated stable rooms are included. Setup, resource checks and Close/EOF teardown are outside the four-second traffic phase. Each series uses fresh processes in control/candidate/candidate/control order. Throughput is a completed-delivery rate, not a fixed delivered-work comparison.

| Placement | Build | Deliveries/s, repetitions | Server CPU µs/delivery, repetitions | p99 histogram upper bound |
|---|---|---:|---:|---:|
| Process CPU masks only | Control | 877,622; 866,572 | 2.089; 2.073 | 33.554 ms; 33.554 ms |
| Process CPU masks only | Candidate | 1,087,210; 878,059 | 1.693; 2.098 | 16.777 ms; 33.554 ms |
| Identified native/Tokio workers pinned | Control | 1,172,798; 1,092,602 | 1.642; 1.678 | 16.777 ms; 16.777 ms |
| Identified native/Tokio workers pinned | Candidate | 1,182,707; 1,148,704 | 1.650; 1.662 | 16.777 ms; 16.777 ms |

The first candidate run looks promising, but its second run is essentially level with the controls. Client CPU usage also differs substantially between those runs (21.78 versus 9.12 CPU-seconds), so do not select the favorable one. The second series explicitly pins every identified native I/O worker and Tokio client worker to remove migration of those workers as an uncontrolled variable; the temporary runner verifies those affinities at gates. The CPU-cost ranges then overlap, with only about 0.26% difference between the means. No repeatable CPU saving or improved latency bucket is established.

Both series remain included. The pinned and unpinned series occur at different times, and pinning both server and client changes the overall configuration; their difference is **not** claimed as a quantified pinning-only speedup. Desktop scheduling/task placement/frequency remain possible sources of variation. Pinning OS threads also does not prohibit Tokio task migration. No kernel/cache-line root cause is established by this screen.

All eight ordinary cases pass full submission/body/FIFO, exact counter and normal Close/resource checks. They verify **33,325,056 timed fanout deliveries**, separate from the instrumented profile. No 100,000-user fairness claim, C++ comparison, saturation guarantee or long-soak result follows from this experiment. The candidate is rejected before expanding to expensive population/pressure comparisons.

## Kept runner controls

`python3 scripts/inspection.py` now accepts:

- `--pin-workers`: passes the native relay's existing startup policy and verifies its actual I/O thread affinity. This is recorded as `method: server_startup`.
- `--pin-client-workers`: pins each known client I/O worker to one CPU inside the existing `--client-cpus` mask. It requires explicit `--client-workers`; this benchmark intervention is recorded as `method: harness`.

The JSON records `thread_affinity` and the runner verifies every recorded singleton affinity again at mixed/done gates. Main, management and other threads retain their process masks; this does not pin every process thread or async task. Without either option, the field is empty and placement behavior is unchanged. A CPU mask alone is not presented as per-thread pinning. The helper refuses insufficient CPUs or a mismatched thread census instead of claiming successful placement.

`pin_child_workers` moves, with an AST-identical body, from capacity into the existing shared `scripts/benchmark.py` utility. `capacity.pin_child_workers` remains an alias, so existing tooling imports continue working. No new affinity implementation, duplicate helper, dependency or platform policy is introduced.

Verification of the final public paths uses short **two-second** integration cases:

- Pinned inspection: four native startup-pinned workers on 0–3; eight harness-pinned client workers on 4–11. Sixteen stable rooms, 512 fanout readers, a checked private pair and two churn workers exercise routing/retirement alongside affinity verification.
- Default inspection: no per-thread pinning requested; empty affinity metadata and no native pin flag. Full fanout/counter/retirement checks pass.
- Capacity compatibility: 128 users with the existing native/client pin options, using the shared helper alias. Exact owned census, eight samples/user, counters and retirement pass.
- CLI rejection without `--client-workers` happens before starting any processes. The real TCP ownership regression and Python compilation/source whitespace checks pass as well.

These are feature/integration checks, not performance comparisons. The public runner requests **native startup** placement; the experimental pinned series uses a temporary helper intervention for both processes at ready. Both verify the same measurement-phase affinity, but the recipes are not silently substituted for each other.

## Provenance and reproduction

The paired release binaries, exact socket source versions, test source, build recipe/compiler, runner snapshots, all receipts/logs, profile text and command records are included. `dependencies.json` binds existing clean library/vendor and frozen-client/native provenance. The native candidate base is clean source; all three original dirty C++/relay files remain byte-identical and none is included in its build. The manual native, bridge and static services remain unchanged.

Run `python3 docs/benchmarks/2026-10-18-notification-screen/verify.py` for offline hashes, source/binary pins, all exact counter receipts, fanout delivery totals, affinity records, helper-body identity and final smoke associations. Raw perf data is intentionally excluded; its local hash is in `local-only-profile.json`. Archived executable snapshots are compressed and need executable mode after decompression. Copy archived artifact scripts back under `artifacts/` to replay their recorded root-relative commands. Source snapshots of old shared helpers are retained so their recorded hashes can be audited.

For future tests through the kept runner, explicitly request placement, for example add `--workers 4 --server-cpus 0-3 --client-workers 8 --client-cpus 4-11 --pin-workers --pin-client-workers` to the desired inspection workload. Check physical siblings on another host before reusing these masks. No default changes or relay performance gains are claimed from adding these controls.
