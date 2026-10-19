# Scope disconnect-observer barriers to global recipients

The prior ordering fix correctly placed captured disconnect notices before an
unsubscribe reply, but drained **every** pending departure job while holding the
registry write lock. A 100,000-member ordinary room made that control operation
and unrelated routing pause for hundreds of milliseconds.

The revised barrier drains only jobs targeting the global channel. Ordinary-room
jobs remain asynchronous. In four serial control/candidate/candidate/control
runs, the acknowledgment fell from **377–378 ms to 1.14–1.19 ms**. An unrelated
connection's overlapping self-echo fell from **376 ms to 0.73–1.65 ms**. Exact
notification delivery, unsubscribe ordering, re-subscription and shutdown all
passed. These are short event measurements on loopback, not an endurance or
statistical tail-latency certification.

## Why the smaller barrier preserves FIFO

Every retirement job captures members of exactly one channel: either the departed
peer's room or the global disconnect observers. Ordinary rooms and the global
channel have disjoint members, and a session cannot change its channel.

The barrier walks queued jobs in their existing order and finishes global-channel
jobs, including departures from the global room itself. It retains ordinary-room
jobs in order for their usual 1,024-recipient turns. No recipient can observe the
change in scheduling between these disjoint groups. Previously visited recipients
are not replayed; each job keeps its existing cursor. Publication remains under
the registry lock and advancement under the retirement mutex, in the existing
lock order, so a new departure cannot cross the acknowledgment boundary.

The implementation uses the existing immutable recipient view to classify each
job from its first member. `VecDeque::retain_mut` removes completed jobs in place.
There is no new per-session field, recipient index, epoch, dependency, payload
copy or heap allocation in this selection step. Existing charged views and
shared frames retain their ownership until each job completes. Native socket
barrier output, including its full-queue pump, is unchanged from the control.

The enhanced regression interleaves ordinary-room, global-room and ordinary-room
departures. It proves that all three global notices precede the reply, ordinary
output remains untouched by revocation, both ordinary notices eventually arrive
in order, no notice is duplicated after the reply, and queue credit returns to
zero. It fails against the control because ordinary-room work was drained.

## Remaining bounds

The barrier still holds the registry write lock while scanning pending jobs and
submitting global output. Many queued jobs or many global recipients can still
make this control turn long. This change removes work for disjoint ordinary
rooms; it does not establish a fixed control-turn bound for large global
populations. Existing queue-pressure fallback still finishes all older jobs
synchronously to preserve best-effort delivery order. Queue budgets and
slow-consumer policy remain unchanged.

## Short wire comparison

Each disposable process has 100,000 ordinary recipients, an authenticated global
observer, a departure source and one independent self-echo witness. The witness
runs sequential verified echoes about every millisecond. The client closes the
source and immediately clears observer bit `4`. It checks the old notice before
the reply, no old notice afterward, and a fresh notice after re-enabling the bit.
Both departures are checked at all 100,000 ordinary recipients. The witness is
stopped before the exhaustive audience drain, so its timing covers the control
operation rather than population teardown.

| Serial run | Acknowledgment ms | Worst overlapping witness RTT ms |
| --- | ---: | ---: |
| Control 1 | 377.488 | 376.389 |
| Candidate 2 | 1.135 | 1.654 |
| Candidate 3 | 1.189 | 0.732 |
| Control 4 | 377.856 | 376.761 |

Every row had a witness sample overlapping the command interval. This is one
witness per event; it does not measure every user's RTT during that event. Client
scheduling, socket and kernel time are included. The independent sample and the
acknowledgment are different intervals, so either can be longer.

A separate candidate run with `--queue-messages 1` passed with a **0.601 ms**
acknowledgment. Its millisecond witness had no sample overlapping the shorter
command interval; it supplies queue-limit correctness evidence, not an
additional simultaneous witness-latency measurement.

All five checks verified re-subscription, zero residual queued credit, zero
routing-copy metric, normal shutdown Close `1001`, server exit `0` and no forced
kill. Server CPUs were 0–7; the client used 8–15,24–31. Runs and builds were
serial; no traffic result includes a concurrent build.

## Normal mixed-load screen

The ordinary mixed check has a two-second steady phase; establishing the
population and exhaustively checking cleanup take longer. It uses 100,000
capacity users, 8,192 room readers, 32 private pairs, two inspectors, eight churn
workers at 32 cycles/s and eight offered multicast windows/s. Full commands,
CPU topology, affinity and executable digests are in the raw receipts.

| Metric | Control | Candidate |
| --- | ---: | ---: |
| Server CPU seconds | 8.93 | 9.24 |
| Private deliveries/s | 1,172,683 | 1,276,827 |
| Worst sampled capacity-user RTT, ms | 4.439 | 3.781 |
| Maximum fanout window, ms | 66.714 | 59.811 |

Both successful runs completed all 16 offered windows, verified 524,288 multicast
deliveries, 512 churn cycles and 750,000 capacity departure checks, exited
`[0,0,0]`, and released the measured final resources. This single pair is a normal
workload regression screen; it does not establish a general throughput or CPU
improvement. The targeted four-run comparison establishes the specific
revocation benefit under the measured ordinary-room departure.

The initial candidate mixed attempt stopped during sidecar connection setup with
`Cannot assign requested address (os error 99)`, before the steady phase. Its
receipt and stderr are retained and excluded from timing. The controller cleaned
up its processes; a fresh-port retry passed. No host networking settings were
changed. This failure does not by itself identify a relay defect or prove the
exact cause of client address exhaustion.

The full library suite passed **147 tests in 10.03 seconds**; Clippy with
`-D warnings` and formatting checks also passed. The offline verifier checks the
archived source/build identities and recomputes witness overlap from raw samples.

## Evidence and reproduction

`control-build.json` identifies the prior selected binary from commit `ed8e721`.
`selected-build.json` records the frozen candidate and all source digests.
`selection.json` identifies the byte-identical selected executable. Changed
runtime/test sources are archived before and after; other source comes from
`ed8e721`. Exact wire/mixed fixtures and imported wire helpers are archived as
gzip files. `before.log.gz` contains the failed enhanced regression against the
control; the selected full-suite and Clippy logs are retained separately.

Run the offline integrity and result checker without network traffic:

```sh
python docs/benchmarks/2026-10-19-observer-scoped-barrier/verify.py
cargo test --lib disconnect_observer_notice_cannot_arrive_after_unsubscribe_acknowledgment
```

To repeat a wire event, decompress `wire-check.py.gz` to a temporary file, set
its `ROOT` to the checkout containing the archived helper scripts, and run:

```sh
taskset -c 8-15,24-31 python /tmp/scoped-barrier-wire.py \
  --server /absolute/path/to/frozen/war-relay --output /tmp/NEW-result.json
```

Repeat serially control/candidate/candidate/control using new output filenames;
add `--queue-messages 1` for the constrained-queue check. Adjust both client and
fixture server affinity for the host. A fresh receipt records the modified
fixture hash. Do not overlap builds with timed traffic. No running service or
original uncommitted source overlay was changed by this work.
