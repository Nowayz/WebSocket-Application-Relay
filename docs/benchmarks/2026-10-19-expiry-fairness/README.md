# Bound expired-timer discovery for responsive socket service — 2026-10-19

**Selected:** each native worker discovers at most 1,024 expired timers before processing its ready batch and polling socket readiness again. The same private `READY_QUANTUM` constant names the existing 1,024-ready-entry bound. Remaining overdue timers make the next poll nonblocking through the existing deadline calculation. No deadline rounding, policy timeout change, new queue, lock, allocation, dependency or public configuration is added.

Previously, a worker popped its entire due heap and appended every expired socket to its ready queue before returning to readiness discovery. After a large overdue burst, an otherwise healthy connection could be appended behind thousands of expiry callbacks. Bounding only the ready-processing batch did not avoid that backlog. The selected change bounds expiry discovery too, allowing readiness events to enter between expiration batches. Pending expiration still progresses on subsequent nonblocking turns. Per-source message FIFO, payload owners and existing resource budgets are unchanged.

## Controlled overdue-burst evidence

A clean diagnostic build first populated 100,000 incomplete HTTP connections plus one joined healthy witness. The successful target ramp took 0.871 seconds. SIGSTOP/SIGCONT on that owned disposable process made every two-second handshake deadline overdue at once. Each of eight workers then popped/scheduled 12,343–12,669 expired timers in one turn, taking about 1.96–2.00 ms for timer discovery alone. The first complete independently verified healthy echo after resume took 110.170 ms. Most of that delay is outside the instrumented heap-discovery interval; the test does not attribute all 110 ms to heap operations.

The final ordinary fixture uses uninstrumented frozen binaries, 100,000 incomplete HTTP connections and one joined witness, eight pinned server workers on CPUs 0–7, and its controller on CPUs 8–15. It gives setup a strict three-second bound, uses fresh loopback source prefixes per trial, requests listen backlog 65,535 (subject to the OS cap), and checks the entire population is present with no rejections/handshake errors before suspension. Handshake timeout is four seconds; suspension is 4.05 seconds. The timer policy is otherwise identical for both binaries. The shared read-buffer admission budget is explicitly bounded at 4 GiB to cover the logical pending-handshake reservations; this is not actual RSS or an allocation claim.

After SIGCONT, the stopwatch starts immediately before sending the witness probe and stops only after checking sender, exact body and sequence. It excludes the deliberate suspension but includes process wakeup, client and kernel work. All raw peers must subsequently produce EOF; the witness then routes another complete probe, closes normally, and all checked resources retire. This is a controlled resumption/expiry stress case, not steady-state throughput, 100,000 active-user latency, WAN/TLS performance or a production SLA.

| Ordinary trial | Setup seconds | First verified post-resume echo ms |
| --- | --- | --- |
| control-1 | 0.886 | 112.257 |
| selected-1 | 0.847 | 10.608 |
| selected-2 | 0.906 | 9.836 |
| control-2 | 0.878 | 108.396 |

The repeated control/selected/selected/control comparison reduces this specific observed delay from 108–112 ms to 9.8–10.6 ms, about 90%. Every run verifies all 100,000 expected handshake expirations, all three healthy application probes and normal witness Close/EOF. No server is forcibly killed. Connections, sessions, queue bytes/capacity, variable bytes, read reservations and native arena slots retire to zero; descriptor counts return to baseline. Routing copied bytes and checked unrelated protocol/rejection/close-failure counters are zero. Intentional handshake-error count is exactly 100,000. Each build's SHA-256 is pinned, and ordinary receipts contain no diagnostic timer output.

## Normal mixed-load acceptance

Four separate ordinary runs use two traffic seconds each with 100,000 private users plus 8,192 authenticated fanout observers, eight server workers, disjoint client CPU sets, 1 KiB bodies, four private probes per user per second, 16 offered four-message fanout windows per second and eight churn workers offering 16 lifecycles/second each. No profilers, builds or other test workload overlap timing. Heartbeats, application idle expiry and rate limits are disabled in this fixture; existing message/queue/staging bounds remain active. Full commands and identities are retained in receipts.

| Trial | Server CPU-s | Median user's worst private ms | p99 of users' worst private ms | Worst private ms | Worst fanout ms |
| --- | --- | --- | --- | --- | --- |
| control | 6.20 | 2.253 | 13.912 | 16.047 | 32.876 |
| selected | 6.46 | 3.742 | 16.120 | 18.852 | 37.130 |
| selected-repeat | 6.21 | 2.371 | 14.241 | 18.137 | 32.453 |
| control-repeat | 6.51 | 3.801 | 15.886 | 19.378 | 35.341 |

The first candidate was slower than its control, so the order was reversed. The repeat candidate closely matches the first control's CPU and typical private maximum, while the returning control is slower. These finite overlapping observations do not establish normal-load latency/CPU improvement or equivalence. Selection is for the repeatable overdue-burst responsiveness fix.

Every normal trial verifies 800,000 measured private deliveries, 1,048,576 measured fanout deliveries, all 32 offered windows and all 256 churn lifecycles, plus census/counters/FIFO/body and final retirement. Each of its three disposable processes exits zero. Per-user private statistics have only eight observations, so empirical per-user p99 equals maximum; this is not a long-term tail bound. Raw CSV paths and hashes remain in each receipt and local artifacts; the verifier can optionally check them.

## Regression checks and retained fixture failures

The deterministic boundary test creates 2,049 due timers and one future timer, invokes the real worker scheduling helper, and confirms exactly 1,024 expired entries were consumed while the future deadline remains untouched. It fails when that helper uses the original unbounded loop. All **121 library tests pass in 0.55 seconds**, strict bins/lib/tests Clippy passes, and **65 JavaScript checks pass in about 1.12 seconds**, with all six checked resource gauges retired. Existing shutdown and absolute deadline tests remain included.

An initial five-source-IP sequential ramp was stopped when early sockets were already expiring before the target population was established. Later setup-guard failures are also retained and excluded from successful comparisons. The first candidate fixture sampled zero connections just before the final arena-capacity gauge retired; the corrected final gate waits, boundedly, for every required resource gauge, rather than treating a single non-quiescent snapshot as final. Final ordinary controls and candidates all use that same gate and setup policy. Earlier successful two-second-policy trials are retained as preliminary observations, not mixed into the final four-second-policy table.

The debug shared target initially reused the just-built unbounded counterfactual test binary (0.02-second Cargo finish). The boundary regression caught this. Refreshing copied source timestamps forced recompilation before the accepted library/Clippy/JS results. The stale log is retained; release benchmarks were already frozen through `build-snapshot.py` with `fresh:false`, exact input hashes and distinct control/selected binary hashes. No stale debug result is accepted.

There are eight normal traffic seconds and four 4.05-second controlled suspensions in the final comparison, plus setup, checks and cleanup; no multi-hour campaign was used. Sources, patches, manifests, all successful and rejected receipts, fixture versions and test logs are retained. `expiry-pressure.py` is a Linux-specific reproducibility fixture: from the repository root, pass `--server PATH --users 100000 --source-prefix UNUSED_LOOPBACK_PREFIX --output FRESH.json`; inspect/adapt its recorded CPU masks first. It suspends only its own disposable server and has explicit setup/cleanup bounds. `compare.py` retains the original sequential commands and fresh output paths. Existing manually running services, convenience binaries and three unrelated worktree edits were preserved.

Run `python docs/benchmarks/2026-10-19-expiry-fairness/verify.py` to audit archived receipts, hashes, timer counts, selection boundaries and final resources. Add `--check-local-csv` to verify the original local CSVs and their audience/sample counts.
