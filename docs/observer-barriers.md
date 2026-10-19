# Observer-specific departure barriers

Clearing an authenticated global listener's disconnect bit must put already
captured notices before its acknowledgment. The previous correct implementation
finished pending global-channel jobs, submitting output to every global recipient
while holding the registry write lock. One observer's control command could
therefore stall unrelated users for hundreds of milliseconds.

The revised barrier submits only that observer's still-unvisited notices, in the
shared departure FIFO's order. A monotonically increasing publication sequence
and a per-peer watermark prevent later worker visits from replaying those notices,
even after re-subscription. Already-visited recipients are not sent duplicates.
Ordinary rooms and other observers retain their normal 1,024-recipient turns.
The acknowledgment also uses the ready-output barrier path: a full writable
one-slot queue can drain before reserving the next message slot.

Publication still takes the registry lock and then the existing retirement mutex;
advancement takes only the retirement mutex. The barrier holds both in that same
order, so no departure can publish across its captured boundary. Watermark access
is serialized by the retirement mutex. The counter survives an emptied queue;
exhaustion uses ordered synchronous fallback rather than wrapping. No additional
lock, recipient index, payload copy or per-barrier allocation is introduced.
Published shared bodies and recipient views keep their existing charges until
all remaining work finishes. Application-message fanout does not read watermarks.

A compiler-layout census on the measured host found `Peer` grew from 80 to 88
bytes; `Budget` stayed 32 bytes and `RoomFanout` stayed 112 bytes. The explicit
watermark cost is eight bytes per peer, about 0.76 MiB for 100,000 peers, plus one
counter per relay. This is a measured layout cost, not an exact RSS prediction:
allocator granularity and other retained storage also affect process memory.

## Short 100,000-observer event comparison

Four serial control/candidate/candidate/control runs used eight pinned server
cores and separate client cores on the same host. Each process held 100,000
audience observers plus the tested observer, a departure source and one independent
echo witness. The fixture checked the old notice before the unsubscribe reply,
no late duplicate, a fresh notice after re-subscription, all audience departures,
zero residual queued credit, zero recorded routing-copy bytes and 100,000 shutdown
Close `1001` receipts. Every server exited normally without a forced kill.

| Run | Unsubscribe acknowledgment, ms | Worst witness RTT overlapping the command, ms |
| --- | ---: | ---: |
| Control 1 | 348.204 | 347.962 |
| Candidate 2 | 0.971 | 0.663 |
| Candidate 3 | 0.961 | 0.669 |
| Control 4 | 343.179 | 343.011 |

The median acknowledgment fell from 345.692 to 0.966 ms, about 99.72% lower for
this event. Every row included an overlapping witness sample. The witness's
maximum across its entire short sampling period was 1.247–2.486 ms for the
candidates. These are loopback command/event measurements including client and
kernel scheduling, not server-only timings, endurance results or every user's
latency during the event.

The original one-slot fixture passed revocation/re-subscription but then failed
on both control and candidate: it published two later departures before draining
their recipients. That required absorbing a burst larger than the configured
one-message queue. Those complete cases remain failed evidence, excluded from the
table. The maintained fixture drains each follow-up departure before publishing
the next. Its 100,000-observer candidate check with `--queue-messages 1` passed
at 0.926 ms and verified all 100,000 shutdown Close receipts. Optional
`--burst-teardown` retains the separate queue-pressure diagnostic.
A repeat with diagnostic frame details confirmed a recipient Close `1013`,
`Try Again Later`, during that burst. It remains a failed pressure case, not
evidence that a one-slot queue can absorb arbitrary concurrent departures.

## Ordinary mixed-load regression screen

Four more serial runs used 100,000 capacity users, 8,192 room multicast readers,
32 private pairs, two inspectors, eight churn workers at 32 cycles/s and eight
offered four-message fanout windows/s. The steady phase was two seconds; setup,
proof delivery and exhaustive cleanup take additional wall time. Each capacity
user supplied eight observations, so its reported p99 equals its observed maximum.

| Run | Server CPU, s | Private deliveries/s | Median user's observed maximum, ms | Worst user, ms | Worst fanout window, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.37 | 1,335,673 | 0.928 | 3.856 | 64.301 |
| Candidate 2 | 9.31 | 1,058,713 | 1.146 | 4.782 | 75.108 |
| Candidate 3 | 9.36 | 1,288,932 | 1.029 | 4.673 | 62.008 |
| Control 4 | 8.95 | 950,144 | 1.185 | 11.754 | 78.867 |

Every run completed all 16 offered windows, 524,288 multicast deliveries, 512
churn cycles and 800,000 checked capacity-user observations, along with the
capacity departure matrix and final resource checks. All process exit codes were
`[0,0,0]`. Intended-send worst latency, which includes generator scheduling,
ranged from 5.950–12.816 ms for controls and 6.815–8.436 ms for candidates.

The ranges overlap. Candidate 2 had lower private throughput and higher sampled
latency than Control 1; the reverse-order repeats did not establish a consistent
ordinary-load regression or gain. These four short screens do not certify a
long-term per-user p99. Selection is justified by the repeatable targeted barrier
benefit with checked semantics, not a claimed general throughput improvement.

## Limits and further work

The barrier still scans pending global recipient views under the registry write
lock. Its cost depends on queued jobs and their unvisited membership, rather than
being a fixed-time operation. Many pending views can still produce a long control
turn. Queue-budget refusal still invokes synchronous best-effort fallback;
slow-recipient and exhausted-credit policies are unchanged. Keep enough metadata
headroom for captured membership views.

Further optimization should begin with a short screen that establishes a remaining
bottleneck, such as many pending global jobs or queue-budget fallback. Include
normal mixed traffic and all-user intended-send measurements before adopting a
more complicated membership index or scheduling design.

## Reproduction and source identity

The control is consolidation commit `f58441c`. Frozen release SHA-256 identities:

- Control server: `ca98c5e5fce778706ca7f731f803c7153286e29809983299777da090fa9f41aa`.
- Selected candidate server: `1668b00176a4b9d5cc1610128108c8e335636f79471afb7b8497d5d0b7b39738`.

The first four event rows used the same frozen fixture, SHA-256
`fbcf371f14cb5573bf79af34e24b801c30eaca46dbce00282854d208705bf336`.
The maintained `scripts/observer-barrier.py` derives from that fixture, adds
portable affinity/worker options and helper identities, improves failure output,
and separates sequential from burst teardown. That revision supplies the
one-slot and small CI checks. It uses independent encoded native CBOR, without
the JavaScript library or an alternate wire protocol.

On the measured 32-logical-CPU host, repeat with fresh output names:

```sh
taskset -c 8-15,24-31 python3 scripts/observer-barrier.py \
  --server /absolute/path/to/frozen/war-relay --server-cpus 0-7 \
  --audience 100000 --output artifacts/observer-event.json

python3 scripts/mixed-capacity.py \
  --server /absolute/path/to/frozen/war-relay \
  --capacity-client /absolute/path/to/frozen/capacity \
  --sidecar /absolute/path/to/frozen/inspection \
  --users 100000 --seconds 2 --readers 8192 --scope room --pairs 32 \
  --churn-workers 8 --churn-cycles-per-second 32 \
  --fanout-windows-per-second 8 --pin-client-workers \
  --output artifacts/observer-mixed.json
```

Adjust masks for another host. Use identical frozen clients, serial builds/runs,
and no profiler for ordinary timing. Add `--queue-messages 1` to the event check
for bounded-message correctness. A portable small CI check uses `--audience 2048
--workers 2` and no CPU mask. The full Rust suite passed 208 tests; Clippy with
warnings denied, formatting, all 66 JavaScript tests, vendor provenance and the
maintained-product audit passed. The CI fixture also checks all 2,048 shutdown
Close receipts. No legacy or C/C++ implementation is reintroduced.

The [subsequent snapshot ownership change](retirement-snapshots.md) reuses and
charges overlapping disconnect views once. The footprint and comparison numbers
above describe this report's `28e97de` revision.
