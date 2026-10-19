# Release global listener indexes before bulk shutdown

A short 100,000-global-observer check exposed repeated listener-index searches
while shutting down. The prior server exceeded cleanup's deadline with the
fixture's shortened two-second grace. The production fix is one statement:
release the three listener vectors at terminal shutdown, before their sessions
retire individually.

With the unchanged **ten-second default** grace, both builds verified all 100,000
normal Close frames. Four serial candidate/control/control/candidate runs
reduced the median end-to-end shutdown check from **3.834 s to 2.650 s**, about
**31% less time**. The change adds no dependency, per-peer state or live-routing
allocation.

## Cause, ownership and locking

Global listener flags are indexed by three vectors of peer handles. Removing a
flag searches the matching vector for that peer, then uses `swap_remove`.
Individually removing a large population can perform quadratic search work.
Terminal shutdown already suppresses departure notices, so these listener
indexes no longer serve notification routing during bulk retirement.

`begin_shutdown` now publishes the terminal stopping state, replaces the indexes
with empty vectors under the registry write lock, releases that guard, and then
clears the pending retirement queue as before. Registry and retirement guards
are not nested by this new statement. The normal removal path subsequently
clears each retiring peer's flags against empty indexes. Registrations and the
registry retain live sessions until their transports retire. This differs from a
reversible manual drain, whose listener indexes remain available.

The regression enables all three listener classes on 1,025 global sessions,
checks that terminal shutdown releases index capacity before those registrations
are dropped, calls shutdown twice, then proves final session/queue cleanup and
release of a weak peer reference. It fails against the control and passes with
the fix. The full library suite passed **148 tests in 10.04 seconds**, including
existing shutdown/admission/fairness checks. Clippy with `-D warnings`, formatting
and the offline archive verifier passed.

## Short real-socket checks

The fixture authenticates 100,000 global-channel recipients and enables their
disconnect flag. A separate observer unsubscribes during an ordinary source's
departure; an unrelated witness performs verified self-echoes. It checks the
captured notice before the unsubscribe reply, no late old notice, and a fresh
notice after re-enabling. Every audience member receives both source departures
and the later observer/probe departures in order. Before SIGTERM it checks
100,000 remaining sessions, zero queued credit, zero routing-copy metric and the
existing error-counter gates.

The strict fixture then requires a server-sent Close **1001 at every one of the
100,000 remaining sockets**, closes client descriptors and reaps the server. A
successful process exit alone cannot pass. All timed runs and builds are serial;
setup is outside the shutdown interval. Server CPUs are 0–7 and client CPUs
8–15,24–31; environment and exact commands are retained.

| Serial run | Shutdown check ms | Verified normal Close frames | Server exit |
| --- | ---: | ---: | ---: |
| Candidate 1 | 2,653.635 | 100,000 | 0 |
| Control 2 | 3,855.727 | 100,000 | 0 |
| Control 3 | 3,811.529 | 100,000 | 0 |
| Candidate 4 | 2,645.927 | 100,000 | 0 |

`shutdown_check_elapsed_ms` measures SIGTERM through client Close verification,
client-descriptor cleanup and server reaping. It includes client work and is
**not server-only shutdown CPU or runtime**. The same full operation is measured
before and after. Two runs per build demonstrate this local cleanup benefit;
they are not an endurance test or a production latency certification. A
100,000-global-session fixture also needs population setup, so its total wall
time is longer than the measured shutdown interval.

## Deadline and latency limits

Two negative results are deliberately retained:

- `global-1-control.json.gz` used the earlier two-second fixture, checked only
  one shutdown Close, and ended with server exit `1` and `service shutdown
  exceeded its deadline`. It motivated investigation but is not part of the
  strict full-matrix timing comparison.
- `shutdown-2-candidate.json.gz` strengthened verification to every client while
  keeping that two-second grace. The server exited `0`, but only **72,846** normal
  Close frames were verified before an unexpected EOF. This remains a failed
  strict check. A successful server exit does not prove all clients received a
  Close. The default-grace comparison explicitly uses ten seconds on both
  builds; no server default or operator policy was changed.

Size shutdown grace for population cleanup. This fix does not promise that
100,000 responsive sessions all receive a Close within two seconds.

The original global-observer unsubscribe barrier remains synchronous. These
checks still measured about **338–350 ms** for its acknowledgment and a similar
unrelated witness stall. This change improves terminal cleanup; it does not
claim an unsubscribe-latency gain or resolve that remaining live-control issue.
The earlier scoped-barrier optimization still keeps disjoint ordinary-room
jobs asynchronous.

## Evidence and reproduction

`control-build.json` identifies the prior selected build from commit `fec8fe6`.
`selected-build.json` records every source digest and frozen executable identity;
`selection.json` identifies its byte-identical selected copy. The changed runtime
and test sources are archived before and after. The exact three fixture versions
and imported wire helpers are preserved, including the weaker initial fixture
and failed short-grace strict check. `environment.json` records compiler, kernel
and CPU information. Raw JSON and test logs are gzip-compressed.

The offline checker runs no load:

```sh
python docs/benchmarks/2026-10-19-global-shutdown-indexes/verify.py
cargo test --lib shutdown_releases_listener_indexes_before_retiring_global_sessions
```

For a fresh strict run, decompress `shutdown-wire-default.py.gz` to a temporary
file and set its `ROOT` to a checkout containing the archived helper scripts:

```sh
taskset -c 8-15,24-31 python /tmp/global-shutdown-wire.py \
  --server /absolute/path/to/frozen/war-relay --output /tmp/NEW-result.json
```

Run serially candidate/control/control/candidate using distinct output paths.
Adjust both fixture server and client affinity for the host. A fresh receipt
records the edited fixture hash. The ten-second grace is part of this experiment;
do not silently replace it with the two-second diagnostic. Existing services
and original uncommitted source overlays were preserved.
