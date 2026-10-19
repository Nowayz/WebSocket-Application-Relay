# Current performance guide

Use this page to find the selected implementation and choose a short test. The detailed [performance history](performance.md) and individual reports retain the evidence; experimental binaries and attractive isolated results are not automatically selected runtime changes.

## Selected implementation and evidence

| Component | Selected behavior | Evidence |
| --- | --- | --- |
| Native Rust relay | Mio readiness workers; immediate private sends; shared immutable multicast bodies; bounded vectored output; 1,024 ordinary-room recipient visits per source turn | [100,000-user mixed-load selection](benchmarks/2026-10-18-mixed-100k-fairness/README.md) |
| Native deferred output | Single queued frame uses the small borrowed writer; larger queues retain bounded vectored batching (`266e5b1`) | [Selected writer and tradeoffs](benchmarks/2026-10-18-single-queued-write/README.md) |
| Native scalar control storage | Full-width unsigned replies fit the 14-byte inline prefix; Frame remains 48 bytes on the measured x86-64 build. This removes a large-ID reply allocation, not a general forwarding speedup | [Selected allocation/wire evidence](benchmarks/2026-10-19-inline-u64-control/README.md), [short 100k check](benchmarks/2026-10-19-current-100k-short/README.md), [earlier constructor/workload tradeoffs](benchmarks/2026-10-18-inline-u64-screen/README.md) |
| Native startup failure handling | Fallible worker/listener reservations; partial OS-thread startup stops and joins earlier workers within the cleanup deadline before returning its error | [Reservation failures](benchmarks/2026-10-19-worker-reservation/README.md), [actual child-only thread-limit fault](benchmarks/2026-10-19-worker-startup-cleanup/README.md) |
| Native secret loading | Invalid UTF-8 environment secrets fail before binding; absent/empty behavior is preserved and errors omit secret contents. Embedders handle `load_secrets()` as a Result | [Configuration and release regressions](benchmarks/2026-10-19-config-secret-encoding/README.md) |
| Native timer updates | Identical existing `Instant` deadlines stay in place; changed deadlines and cancellations use the existing indexed heap | [Keepalive census, two-second 100k comparisons and build-cache preflight](benchmarks/2026-10-18-unchanged-timers/README.md) |
| Native text validation | Longer definite CBOR text uses borrowed SIMD UTF-8 validation; short canonical strings retain minicbor. Bounded validation and shared payload ownership remain intact | [Selected text validation and tradeoffs](benchmarks/2026-10-18-simd-text-validation/README.md) |
| Public CBOR constructors | Reserve actual header sizes for text/envelope/reply values; native forwarding retains borrowed Frames | [Helper microbenchmarks and clone-heavy tradeoffs](benchmarks/2026-10-18-helper-sizing/README.md) |
| Allocator | Existing system allocator; no default jemalloc preload | [Four-wave retirement/allocator screen](benchmarks/2026-10-18-churn-retention/README.md) |
| Management page selection | Empty pages avoid allocation; populated pages use bounded selection batches (`3ee013c`) | [Allocation and locking tradeoffs](benchmarks/2026-10-18-batched-channel-page/README.md) |
| Management authentication | Compare existing Authorization token bytes directly with the configured secret using the unchanged constant-time helper, so correctly supplied UTF-8 tokens can match | [Header and direct HTTP regressions](benchmarks/2026-10-19-admin-token-bytes/README.md) |
| Legacy bridge | CBOR upstream, upstream TCP_NODELAY, 4 KiB initial upstream read buffer; direct single-buffer binary, text, variable-write, byte-command and scalar-command encoding; existing frame/message limits retained | [Delayed-ACK fix](benchmarks/2026-10-18-bridge-upstream-nodelay/README.md), [connection memory](benchmarks/2026-10-18-bridge-read-buffer/README.md), [binary encoding](benchmarks/2026-10-18-bridge-fused-encoding/README.md), [text encoding](benchmarks/2026-10-18-bridge-text-encoding/README.md), [variable writes](benchmarks/2026-10-19-bridge-variable-write/README.md), [byte-valued commands](benchmarks/2026-10-19-bridge-byte-request/README.md), [scalar commands](benchmarks/2026-10-19-bridge-scalar-requests/README.md) |
| Bridge join storage | Release the frontend join view after encoding the independent subscription; text joins transfer their byte owner without copying | [Binary-join retention fix](benchmarks/2026-10-19-bridge-join-owner/README.md), [text-join ownership](benchmarks/2026-10-19-bridge-text-join-owner/README.md) |
| Bridge upstream lifecycle | Both legs bound fragment parsing to 256 frames per poll. Upstream decoder failures produce mapped legacy Close codes; connection failures/timeouts produce 1013, with bounded cleanup and slot reuse | [Frame errors](benchmarks/2026-10-19-bridge-upstream-frame-errors/README.md), [fragment yields](benchmarks/2026-10-19-bridge-upstream-fragments/README.md), [unavailability and recovery](benchmarks/2026-10-19-bridge-upstream-unavailable/README.md) |
| JavaScript connection cleanup | Remove library-owned live handlers on final socket Close before notifying subscribers; pending and queued controls retire without another send | [Handler ownership and real-socket checks](benchmarks/2026-10-19-js-final-close-cleanup/README.md) |
| JavaScript receive boundary | Binary welcome/data only; reject WebSocket text before typed-array length coercion. Pending/queued controls retire through existing error handling | [Source, real-socket and browser-bundle checks](benchmarks/2026-10-19-js-binary-wire/README.md) |

The table describes selected committed source behavior. Uncommitted prototypes can affect builds from a working tree; inspect its diff or use a clean snapshot for a comparison against the selected source. The frozen native baseline predates the deferred-writer and management-page changes; a running historical binary does not acquire later source changes.

The frozen native baseline is runtime revision `279c3c1`, SHA-256 `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`. The frozen bridge baseline is code revision `4c9113a`, SHA-256 `bb2392f53bd8fe1bdff47f0a8e790b366269400e259839565bdeb4573f9f8bf1`. These identify measured artifacts, not the repository's latest documentation commit. A new build must be identified by its own source and executable hashes.

The primary mixed-load comparison has 100,000 private users plus 8,192 fanout readers, churn and inspection. Selected Rust's median user p99 is 2.03 ms, versus 11.86 ms for the accepted modern C++ reference; sampled native RSS is about 115 MiB versus 70 MiB. Those are finite loopback observations under that report's exact workload, not universal language rankings or total machine memory requirements. Kernel sockets and generators consume additional memory. The original upstream C++ implementation is a separate reference with its original protocol; follow [the C++ comparisons](../cpp/BENCHMARKS.md) rather than assigning modern C++ results to it.

The [six-second consolidated 100,000-user acceptance](benchmarks/2026-10-18-current-100k-acceptance/README.md) completes all 96 offered fanout windows in a six-second phase. It records 2.4 million private observations, per-user plots, exact fanout samples and full retirement. Private user maxima have population p99 4.92 ms and worst 7.76 ms; fanout recipient maxima reach 64.84 ms. This is finite acceptance, not a paired speed comparison or equally low latency across traffic classes.

## Choose the smallest test that answers the question

| Question | Test | Measured phase |
| --- | --- | --- |
| Does a local routing/output change merit further testing? | Two-second `scripts/inspection.py` screen below; compare fresh control/candidate processes sequentially | Two seconds plus setup/retirement; rejects regressions, does not establish population fairness |
| Does a change hurt private users while a hot room broadcasts? | `scripts/mixed-capacity.py --seconds 2` with the full mixed-load options below | Two seconds; eight observations/user at 4 Hz. Use six seconds only when 24 observations/user answer the next question. |
| Do the JavaScript clients still interoperate and release resources? | `./scripts/check-js.sh` | One disposable fixture; full protocol/legacy/idle suite plus six zero-resource gauges. Build/install time is additional; honors `CARGO_TARGET_DIR`. |
| Do unread consumers retire while healthy traffic survives? | [Selected-writer pressure fixture](benchmarks/2026-10-18-selected-writer-pressure/README.md): 128 stable users plus eight unread fanout recipients | 25 seconds of stable probes with a ten-second pressure phase; bounded Close/retirement checks add wall time |
| Do repeated population bursts retire correctly? | `scripts/capacity.py --users 16384 --room-size 2 --waves 4 --seconds 1 --active-percent 10` | Four one-second traffic phases, with checked joins/closes between |
| Does bridge cancellation retire both legs while native traffic survives? | [Disposable shutdown probe](benchmarks/2026-10-18-bridge-shutdown/README.md) | Small joined/incomplete populations; current transport-cancellation policy |
| Did input/control handling change? | `scripts/input-corpus.py --workers 4 --clients 8` | Finite 776-case independent corpus |
| Do stalled upgrades or Ping-only prejoin peers release admission? | `scripts/handshake-pressure.py` and `scripts/join-pressure.py` | Fixed five-second HTTP / two-second join-deadline fixtures; [checked examples](benchmarks/2026-10-18-stalled-ingress/README.md) and [unacknowledged Close](benchmarks/2026-10-18-unacknowledged-close/README.md) |

Setup, proof scans and exhaustive departure/Close checks add wall time outside the measured phase. In particular, six seconds of offered mixed traffic does not mean a six-second complete test. Extend a test only when the result needs it; a short screen can reject a clear regression without pretending to prove a long-term tail guarantee.

For copied benchmark archives, prefer `scripts/build-snapshot.py --snapshot PATH --target-dir CACHE --output NEW_DIRECTORY --bin war-relay` (or `--example inspection`). It refreshes copied inputs, requires Cargo's structured artifact message to confirm recompilation, checks unchanged source contents and freezes the executable with hashes and logs. It refuses checkouts and reused outputs. See the [command, real stale-cache regression and acceptance](benchmarks/2026-10-18-snapshot-builder/README.md).

Snapshot, cache and output directories must be separate with no ancestor/descendant overlap after resolving aliases. The [directory-overlap regression](benchmarks/2026-10-18-snapshot-directory-overlap/README.md) closes the previously allowed snapshot-inside-cache case, so a full cache clean cannot erase the copied source. Rejection occurs before timestamp refresh or Cargo invocation.

Prepare snapshots in an explicitly fresh directory, with shell error propagation enabled (`set -euo pipefail`). Stop if `mkdir` fails; never continue extracting into an older experiment's scratch directory. Keep historical source snapshots, receipts and frozen binaries intact. The build helper freezes existing copied inputs; it cannot undo an archive extraction that already overwrote the wrong directory.

For manual builds sharing a target directory, refresh the copied source timestamps (including vendored sources) or clean the relevant artifacts before compiling. Archive timestamps can be older than cached outputs: the [timer experiment](benchmarks/2026-10-18-unchanged-timers/README.md) caught a diagnostic binary reused as an untouched control. Check the compile log, freeze the binary, record its hash and verify diagnostic identity before timing. Cargo success alone does not prove source freshness. Keep source edits and all builds using the shared cache sequential.

For a first routing/output screen, build the intended sources and use a fresh output path:

```sh
cargo build --release --bin war-relay --example inspection
python3 scripts/inspection.py \
  --server target/release/war-relay \
  --client target/release/examples/inspection \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers \
  --output artifacts/new-build-short-mixed.json
```

Use `--value-kind ascii` or `--value-kind utf8` for equal-size text values; default `bytes` preserves the earlier byte-string workload and omits the extra client flag. Templates are prepared before timing, and complete type/header/body bytes remain checked. The [typed workload screen](benchmarks/2026-10-18-value-kind-workloads/README.md) records all three modes and the UTF-8 validation hotspot. The mixed runner accepts the option for its sidecar too; capacity probes remain byte strings.

The inspection fixture accepts `--private-payload-bytes N` (default 1,024; minimum eight for its sequence marker) while fanout values stay at 1 KiB. The [short mixed-size coverage](benchmarks/2026-10-19-private-payload-workloads/README.md) verifies header-boundary accounting, odd UTF-8 contents and old-client default compatibility. Its unpaced 1/64 KiB screen exposes size-sensitive tails without establishing a scheduler change or 100k fairness result. The mixed-capacity runner does not forward this option.

Use `--pairs 0` for the corresponding fanout-only screen. The CPU masks above are for the recorded host; choose disjoint available CPUs on another machine. Freeze both executables before comparing variants, record their identities, and run control/candidate/candidate/control sequentially. Build outside timed traffic. A short screen can reject a candidate; favorable results still need the relevant fairness or pressure fixture. It does not prove 100,000-user behavior. Add `--profile` only for a separate diagnostic run; instrumented/profiled timings are excluded from ordinary comparisons.

The selected-build [CPU profile](benchmarks/2026-10-18-selected-cpu-profile/README.md) and [clock call census](benchmarks/2026-10-18-clock-call-census/README.md) distinguish sampled cycle costs from call frequency. Dominant explicit clock calls check connection policy and start queued-frame deadlines; these times serve different purposes. [Admission retry counts](benchmarks/2026-10-18-admission-retries/README.md) are low in the measured fixture, but do not quantify successful atomic cache-coherence cost. None of these diagnostics establishes a performance gain or justifies weakening deadlines or exact admission.

For population fairness, build deliberately from the source tree you intend to measure, including or excluding local changes explicitly:

```sh
cargo build --release --bin war-relay --example capacity --example inspection
python3 scripts/mixed-capacity.py \
  --server target/release/war-relay \
  --capacity-client target/release/examples/capacity \
  --sidecar target/release/examples/inspection \
  --users 100000 --seconds 2 --readers 8192 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency \
  --output artifacts/new-build-mixed.json
```

The command above omits optional stage-clock instrumentation and uses two traffic seconds. Use `--seconds 6` when the extra observations are needed. Add `--latency-stages` only in a separate clock-decomposition diagnostic; its client-side instrumentation is not an ordinary comparison setting. Keep `--exact-fanout-latency` when exact observer samples are required.

The inspection and capacity runners reserve their receipts with exclusive creation before launching the relay. Reusing an existing receipt path fails without starting the server or replacing prior evidence; select a fresh name instead. Inspection profiled runs also check their derived `.perf.data` path before launching the relay, including dangling symlinks, and report the conflicting artifact path. `python3 scripts/test-inspection-output.py` checks both receipt guards, inspection profile collisions and fresh inspection startup-failure receipts in under a second on the recorded host. Receipt reservation is exclusive; the profile check is preflight, so keep the existing sequential-run discipline and give profiles distinct basenames.

If the OS refuses to execute a server after receipt reservation, inspection and capacity now save the failed JSON outcome instead of leaving an empty file; null `server_exit_code` means no server handle was created. The [bounded exec-failure regressions](benchmarks/2026-10-19-benchmark-exec-failure/README.md) cover missing execute permission/interpreters and confirm that no load client starts.

Inspection profiler shutdown now handles a SIGINT timeout with one bounded kill/reap step, records forced termination as failure, and continues client/server cleanup and receipt writing. The [controlled and real-child checks](benchmarks/2026-10-19-inspection-perf-cleanup/README.md) take one traffic second each; `python3 scripts/test-inspection-cleanup.py` exercises the timeout without sleeping.

The mixed-capacity runner also rejects existing receipt, private CSV, timeline and process-log paths before launching its children, plus fanout CSV/profile paths when those modes are enabled. This is a preflight guard for sequential runs, not a cross-process artifact lock. `python3 scripts/test-mixed-output.py` checks regular-file and dangling-symlink collisions for all nine artifact kinds.

`scripts/mixed-capacity.py --workers N` selects 1–8 native/C++ I/O workers within this recorded-host fixture's fixed server CPU pool 0–7; default eight preserves previous commands. The gate verifies the requested number of distinct pinned workers. This is a fixture constraint, not a production relay limit. Client masks stay unchanged. Compare counts using one frozen executable and reversed-order repeats.

The [current 100k observer/churn comparison](benchmarks/2026-10-19-mixed-worker-count/README.md) passes the offered 16 fanout windows/s with eight workers. Four workers deliver correctly but miss that offer in both short runs and have much worse private-user tails. This does not make eight a universal count for all hardware, room sizes or private-only traffic; the four-worker deployment example has a different traffic/member envelope.

Use a fresh output name. This measures the new executable, not a reproduction of a frozen historical binary. The runner's CPU masks are specific to the recorded Linux host; inspect topology before adapting them. Run comparisons sequentially, with matched settings, fresh processes and no concurrent builds/profiles.

## Read the result correctly

- Treat each user equally. A percentile across users' individual latency statistics is different from a pooled packet percentile. With eight observations (two seconds) or 24 (six seconds), nearest-rank per-user p99 equals its observed maximum. The runner reports this explicitly in `latency_sampling`.
- Preserve offered versus completed fanout windows, pre-send generator lag, exact counts, failures and cleanup. Eventual delivery alone does not prove responsiveness. Stage clocks include client scheduling/kernel work; they do not isolate relay-hop latency.
- Add `--pin-client-workers` only as an explicit comparison setting. It records and verifies each client I/O thread's affinity; a process mask alone is not individual pinning. The [short comparison](benchmarks/2026-10-18-mixed-client-pinning/README.md) improves fanout-window maxima at a CPU cost, with overlapping private-user tails. Defaults remain unchanged.
- Logical queue/read-buffer admission budgets are not an RSS cap. Zero routing copied bytes refers to the checked native forwarding path; control encoding, the compatibility bridge and intentional retention compaction have separate ownership/copy behavior.
- Nonzero RSS after all clients leave is not sufficient evidence of a leak. Check owned descriptors, live gauges and exact closes first; the [retirement study](benchmarks/2026-10-18-churn-retention/README.md) distinguishes those from allocator retention.

## Recent alternatives left unselected

Search these decisions and the detailed history before building another prototype. Describe what has changed in the workload, implementation or evidence that could overturn the previous result. A new label for the same layout, queue or accounting idea is not a new experiment. The [current output-accounting review](benchmarks/2026-10-19-output-accounting-review/README.md) records a fresh profile and stops a redundant layout screen before timing.

| Experiment | Decision and reason |
| --- | --- |
| [Cached ready-slot hints](benchmarks/2026-10-18-ready-slot-screen/README.md) | Token checks and compaction/reuse tests pass, but larger ready entries and fallback logic do not establish a full-population win: CPU/intended fanout delays increase while private tails vary. |
| [Skip discarded poll-timeout calculations](benchmarks/2026-10-18-ready-poll-timeout/README.md) | Overlapping CPU costs; no consistent joint latency/throughput improvement. |
| [Specialized internal token hashing](benchmarks/2026-10-18-token-hash/README.md) | Initial fanout improvement did not hold against returning control; mixed traffic included CPU and latency regressions. |
| [Buffered fanout batches](benchmarks/2026-10-18-buffered-fanout/README.md) | Lower CPU/fanout delay did not establish consistently better private-user tails. |
| [512-recipient quantum](benchmarks/2026-10-18-short-fairness-quantum/README.md) | Worsened private-user tails; retain 1,024. |
| [256 KiB input byte budget](benchmarks/2026-10-19-read-byte-budget/README.md) | Diagnostic confirms large-private turns shorten from sixteen messages to four; two-second reversed-order comparisons do not establish a repeatable latency/CPU benefit. Retain the existing frame/recipient-work quotas. |
| [Phase-local handshake lease](benchmarks/2026-10-19-connection-phase-lease/README.md) | Shrinks Connection from 520 to 504 bytes and saves about 2 MiB of requested heap at 131,072 slots, but both candidate median user maxima/CPU exceed both controls and normal RSS shows no saving. Keep the current layout. |
| [Direct CBOR byte-string parsing](benchmarks/2026-10-18-direct-bytes-screen/README.md) | Faster isolated parsing did not improve measured relay performance. |
| [Socket recv substitution](benchmarks/2026-10-18-socket-recv-screen/README.md) | Lower throughput and higher CPU per delivery in the short screen. |
| [Inline cached WebSocket headers](benchmarks/2026-10-18-frame-header-cache/README.md) | Larger queued metadata; lower fanout rates and higher CPU per delivery in both short candidate runs. |
| [Notification/counter-layout screen](benchmarks/2026-10-18-notify-metrics-screen/README.md), [earlier padded counters](benchmarks/2026-10-16-counter-layout-study.json), [striped counters](benchmarks/2026-10-16-counter-stripes-study.json) | Zero redundant notification swaps in the diagnostic; cache-line separation and stripes did not establish repeatable whole-workload benefits. Samples near atomic instructions are not proof of false sharing. |
| [Completed-credit batching](benchmarks/2026-10-18-completion-credits/README.md) | Accounting and ownership checks pass, but mixed and fanout-only screens do not establish a repeatable gain. |
| [Notification queue handoff](benchmarks/2026-10-18-pending-lock-screen/README.md), [later queue-swap repeat](benchmarks/2026-10-19-notification-drain-screen/README.md) | Fewer measured lock collisions or shorter mutex holds did not establish a repeatable application benefit. |
| [Observer-index lock split](benchmarks/2026-10-19-observer-lock-screen/README.md) | Registry-release and permission-barrier checks pass, but the small-screen private-tail advantage did not repeat consistently at 100k users with paced churn. The existing worktree prototype remains unselected. |
| [Inline boolean acknowledgements](benchmarks/2026-10-19-inline-bool-screen/README.md) | Both forms already allocate nothing; the fixed 100,000-command screen did not justify changing the existing static body. |
| [Buffered read before timeout](benchmarks/2026-10-18-buffered-read-screen/README.md) | More fanout throughput did not reduce client CPU per delivery; maxima varied. Revival still needs targeted partial-frame/cancellation checks. |
| [Real-context buffered read](benchmarks/2026-10-18-context-read-screen/README.md) | Small CPU gain failed full-population acceptance: sidecar CPU 18.44→19.70 CPU-s and fanout maximum 75.22→83.34 ms. Keep default client reads. |
| [Output-only drive split](benchmarks/2026-10-18-drive-split-screen/README.md) | Byte-string CPU gain did not repeat for text or fanout-only traffic; returning fanout control beat both candidates. |
| [One reader timeout per fanout window](benchmarks/2026-10-18-window-timeout-screen/README.md) | Fanout-only client CPU savings did not justify higher 100k sidecar CPU and worse private-user tails. Keep per-message deadlines. |
| [Bridge receive ownership](benchmarks/2026-10-19-bridge-receive-owner/README.md) | All six real receive sizes shared storage; explicit early input drop left live heap unchanged. No in-place API or early-drop runtime change selected. |

A change earns selection through the required workload and invariants: low and consistent user latency, complete delivery, FIFO, shared payload ownership and bounded resource admission. Neither a microbenchmark win nor a shorter critical section establishes that by itself.

## Management and runtime notes

The source keeps empty management pages allocation-free, following the [initial empty-page fix](benchmarks/2026-10-18-empty-channel-page/README.md). It now selects populated pages in bounded batches (`3ee013c`): about 25% faster first-page selection in the short screen and one allocation instead of three in the measured build, with a larger 144 KB buffer at limit 1,000. See the [allocation, locking and concurrent acceptance tradeoffs](benchmarks/2026-10-18-batched-channel-page/README.md). The running native baseline above remains unchanged.

Frequent management polling can compete with routing as channel count grows. The [two-second scaling and polling-cadence screen](benchmarks/2026-10-18-management-scaling/README.md) compares 1,024/8,192 rooms, private traffic and churn. A slower polling cadence recovered throughput in that fixture but did not consistently improve every latency bucket; measure the application freshness requirement before adding an index or cache.

For full-population HTTP page acceptance, the mixed runner now supports `--http-page-size 1000`. The [short 100,000-user integration report](benchmarks/2026-10-18-http-page-100k/README.md) retains all trials, including a CBOR run completing only 95/96 offered fanout windows. Console summaries now expose offered-load completion separately from correctness; default CBOR inspection is unchanged.

The [short pagination/churn acceptance](benchmarks/2026-10-18-page-churn-acceptance/README.md) checks limits 1 and 1,000 during channel churn and fanout, including a 2,048-room fixture that exercises bounded partition selection. All three two-second traffic phases pass; full traversal is verified at quiescent gates, without claiming a cross-request registry snapshot during writes.

The manifest enables Axum HTTP/query/runtime support without its unused JSON extractor feature. Management responses continue using direct `serde_json` serialization into the existing output buffer; query decoding remains enabled. This narrows the feature surface without claiming a runtime speedup or removal of shared transitive dependencies. All targets, both release binaries, management tests and a two-second concurrent HTTP/CBOR acceptance fixture were checked.

`war_queued_bytes` is exact global admission accounting, including charged fanout membership snapshots as well as in-flight messages. It is a logical reservation gauge, not resident payload memory. Source `c4ba653` clarifies its HELP text; the manual running native baseline retains the older description until a later selected deployment.

Source `266e5b1` uses the existing small frame writer for a single-entry deferred queue and retains the out-of-line bulk writer for larger queues. The [queue-shape, code-size and 100,000-user report](benchmarks/2026-10-18-single-queued-write/README.md) records the selected outlined implementation and unselected inline version. Both selected short mixed runs use less CPU per delivery, while fanout-only results vary. The large selected run passes all 96 offered windows; CPU is neutral, observed private tails lower and fanout maximum slightly higher. Manual running services remain at the frozen identities above.

## Benchmark clients and latency analysis

Exact fanout diagnostics now include bounded controller-stage records, and acknowledgement collection uses a single five-second deadline with reusable batch storage (`57ba85c`). The [short controller screen and 100,000-user checks](benchmarks/2026-10-18-fanout-controller/README.md) show less queued-index overhead but no established whole-relay latency win. The verified client is also the default inspection benchmark executable.

For short inspection/fanout screens, `scripts/inspection.py --fanout-close-window 32` exposes the existing checked departure batch outside timing; default one remains unchanged. The [outbound-header screen](benchmarks/2026-10-18-socket-header-screen/README.md) records the cleanup deadline failure, successful bounded teardown and rejected one-pass native candidate.

The [large-fanout attribution](benchmarks/2026-10-18-fanout-attribution/README.md) separates publisher delay from post-submission waiting and profiles native+sidecar processes independently. Ordinary median post-submission wait is 51.7 ms; the sidecar profile highlights timeout/receive polling and timer costs. That interval includes server, kernel and client work, and the timeout symbol can contain inlined decoding. A buffered-read client experiment needs measurement and absolute-deadline/partial-frame checks before adoption.

For finite fairness analysis, `scripts/plot-user-latency-comparison.py --statistic max` plots each user's worst observed delivery and reports the fraction with all observations below requested `--threshold-ms` values. It reads both private millisecond and exact fanout nanosecond CSVs; default p99 numerical results are unchanged. The [retained six-second population analysis](benchmarks/2026-10-18-user-maxima/README.md) exposes a broad fanout delay in that fixture: every reader has at least one actual-send observation above 50 ms. Those are whole-path observations, not an isolated server-hop measurement or a long-term SLO. Reusing the existing CSVs required no new traffic.

The comparison loader now [rejects inverted latency clocks](benchmarks/2026-10-18-latency-clock-validation/README.md): intended-send selected statistics and maxima cannot be below actual-send values for the same user's deliveries. Nine focused tests and 16 retained-data equivalence cases cover exact/histogram checks and unchanged valid numerical results. The CLI rejects inconsistent inputs before saving plots or summaries; no extra relay traffic was needed.
