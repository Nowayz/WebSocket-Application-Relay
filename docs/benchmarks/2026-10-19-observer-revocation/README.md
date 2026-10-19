# Disconnect-observer revocation ordering

A global observer could receive a captured disconnect notice **after** its
listener-flags command had acknowledged clearing disconnect bit `4`. The
asynchronous retirement FIFO retained the old recipient view, but the command
updated flags without ordering that pending output before its reply.

The new regression failed against the control. A real WebSocket check with
100,000 ordinary recipients independently reproduced `unsubscribe_ack,
late_departure`. This is a correctness fix, with a material control-latency cost.

## Change and limits

When an authenticated global listener clears disconnect bit `4`, drain older
retirement work before changing the flags and submitting the reply. Use the
existing registry-to-retirement lock order. Retirement output never reacquires
the registry. Ordinary private and multicast paths retain their compile-time
selection, shared payloads, existing queue budgets and ordinary retirement quantum.

The barrier uses immediate nonblocking socket output. If a recipient's message
queue is full, it first tries the existing bounded write pump, provided no codec
control output is pending and the oldest frame has not expired. It still rejects
a full queue after that attempt. An intermediate implementation used deferred
output and falsely overloaded a healthy one-message observer queue when its
notice occupied the slot needed for the acknowledgment. That failed experiment
is retained as `wire-one-slot-intermediate.json.gz`; it is not the selected build.

The barrier synchronously drains pending jobs while holding the registry write
lock. It can delay unrelated routing and has no 1,024-recipient control-turn bound.
The default-queue 100k run measured **372.54 ms** acknowledgment latency; the
isolated one-message-queue repeat measured **372.39 ms**. The control replied in
0.020 ms but violated ordering. These results do not meet a consistently low
latency goal during this exceptional operation. A scoped asynchronous barrier
is the next performance problem; this change establishes the ordering contract
without introducing per-peer epochs or an additional recipient index.

Queue exhaustion and slow-recipient policy still apply; this is not a guaranteed
notification delivery protocol. Global-channel room membership receives its own
room departures independently of disconnect-observer flags.

## Short checks

All tests use disposable loopback processes and existing resource policy. No
production listener was restarted and no global sysctl was changed.

| Check | Control | Selected |
| --- | --- | --- |
| Captured departure vs. unsubscribe reply, 100k recipients | Reply, then late departure: failed | Departure, then reply: passed |
| Re-enable observer and deliver a fresh departure | Passed | Passed |
| One-message queue, 100k recipients | Intermediate fix failed | Passed |
| Full library suite | Regression reproduced | 147 passed in 10.04 s |
| Clippy / formatting | — | Passed |

The wire fixture verifies both departures at every ordinary recipient, each
observer notice exactly once, no post-reply old notice, fresh delivery after
re-enable, zero remaining queued credit, zero routing-copy metric, normal
shutdown Close `1001`, process exit `0`, and no forced kill. Setup/cleanup are
outside the measured acknowledgment interval. These are individual event checks,
not an endurance test or a statistical tail-latency certification.

The mixed check has a **two-second steady phase**; population and exhaustive
cleanup make its wall time longer. Each run uses 100,000 capacity users, 8,192
room readers, 32 private pairs, two inspectors, eight churn workers at 32 cycles/s,
and eight offered fanout windows/s. The exact command is in each raw receipt.

| Mixed metric | Control | Selected |
| --- | ---: | ---: |
| Server CPU seconds | 8.97 | 9.34 |
| Private deliveries/s | 1,165,584 | 1,278,201 |
| Worst sampled capacity-user RTT, ms | 9.477 | 4.746 |
| Maximum fanout window, ms | 61.658 | 66.614 |

Both completed all 16 offered windows, verified 524,288 multicast deliveries,
512 churn cycles and 750,000 capacity departure checks, exited `[0,0,0]`, and
released all measured final resource gauges. The single candidate/control pair
has mixed results and does not establish a throughput, CPU or latency improvement.
These ordinary workloads do not exercise unsubscribe during a large departure.

## Evidence and reproduction

`selected-build.json` records every snapshot source digest and executable SHA;
`selection.json` identifies its byte-identical frozen selected executable.
Three changed sources are retained before/after as gzip files. Other source is
commit `6ac231e`; this fix introduces no dependencies. Raw receipts and exact
fixture versions, including imported wire helpers, are archived here.

`wire-before.json.gz` came from an invalid fixture that selected channel bit `1`
instead of disconnect bit `4`; exclude it. `wire-before-v2.json.gz` is the valid
control reproduction. The first successful one-slot run may overlap Clippy and
is correctness evidence only; use `wire-final-one-slot-isolated.json.gz` for its
reported timing. Earlier fixture versions are preserved rather than rewritten.

Run the offline evidence check without network traffic:

```sh
python docs/benchmarks/2026-10-19-observer-revocation/verify.py
cargo test --lib disconnect_observer_notice_cannot_arrive_after_unsubscribe_acknowledgment
```

For a fresh wire run, decompress `wire-check.py.gz` to a new temporary filename,
set its `ROOT` to a checkout containing the archived helper scripts, then run
with `--server /absolute/path/to/frozen/war-relay --output NEW.json`, and repeat
with `--queue-messages 1`. Run serially with CPU sets adjusted for the host; the
archived fixture uses server CPUs 0–7 and client CPUs 8–15,24–31. A fresh receipt
records that edited fixture's hash. Do not reuse an output path or run builds
alongside measured traffic.
