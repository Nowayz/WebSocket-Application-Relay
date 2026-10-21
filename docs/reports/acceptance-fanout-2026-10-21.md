# Acceptance repair and room-fanout remeasurement

This record follows the review of `33b7138` against the pre-Claude baseline
`bd97df1`. It distinguishes server fixes, fixture-contract corrections, and
ordinary measurements from diagnostics. No routing locks, worker placement,
publication quanta or observer-shard counts were reverted.

## Acceptance changes

The concurrent channel index can publish a new room before its first member is
inserted, and a listing can retain a room handle while its last member leaves.
Reading metadata later exposed an ordinary room with zero members. Existing
HTTP and CBOR inspection fixtures rejected that row during churn.

`ChannelSnapshot::of` now reads the retirement mark and membership under the
same per-room read guard, omitting empty or retired ordinary rooms. Full HTTP,
paged HTTP and CBOR listings share this rule. The permanent `re_globl` room
remains visible when empty. Index walks remain concurrent and the entire
listing is not an atomic snapshot. A deterministic regression test covers
both a room awaiting its first member and a retained retired handle; live churn
fixtures exercise the complete server/client path without relaxing their
expectations.

The observer barrier fixture previously required a shared order across source
rooms. The current sixteen-shard design explicitly preserves order within a
room, not across rooms. The fixture now consumes each source's exact ID once
from an expected set, rejects duplicates and unknown IDs, and requires the
set to be empty **before** the unsubscribe acknowledgement. Any later frame
fails immediately. Every audience member is checked independently, including
re-subscription and shutdown. Unit checks verify that reversed order succeeds
but duplicate, unknown, malformed and trailing-byte notices fail. Existing
Rust tests retain same-room FIFO and long-queue/barrier coverage.

## Measurement method

- Ryzen AI Max+ 395, 16 physical cores / 32 logical CPUs, Linux; Rust 1.91.1
  release builds with the repository's thin-LTO configuration.
- Identical frozen native clients and separately frozen pre-Claude, Claude and
  fixed server binaries. Source and executable hashes accompany the receipts.
- Fresh processes, alternating order, four-second traffic phases; server
  workers pinned to CPUs 0–3 within mask 0–7, client workers to 8–15.
- Room tests have 1,024 readers, four-message fanout windows and 1 KiB values.
  The mixed case adds 32 private pairs; the isolated case removes private
  traffic. A private-only case checks that the listing repair preserves the
  ordinary routing path.
- No simultaneous builds or timed trials. Desktop background activity was
  not stopped. Loopback without TLS, with fixture rate limits and heartbeats
  disabled; these are not production capacity guarantees.
- Reported p99 values are histogram upper bounds. RSS is process memory at
  fixture gates, excluding kernel socket storage. Fanout counts deliveries,
  not original publications; combined throughput includes private deliveries.

The first review's three-run mixed fanout median was about 14% lower on
Claude's revision. The follow-up separates isolated fanout from competing
private traffic and repeats both revisions to distinguish a reproducible
runtime regression from placement/scheduling variation. No speculative
hot-path optimization is justified solely by that initial median.

## Dispatch code generation

The first listing repair introduced a separate mixed-workload slowdown even
though the timed fixture performs no inspection. Two successive matched sets
(five and ten runs per binary) showed lower fanout throughput; isolated fanout
and private-only traffic were essentially unchanged. This candidate was not
accepted as the final build.

The pinned release compiler changed its inlining decision. In Claude's binary,
the native `ReceiveBatch::receive_bounded` contained the routing implementation
(12,327 bytes). The first repair instead emitted the general
`Relay::receive_with_totals` (12,684 bytes), without that specialized native
receive-batch body. The metadata collector had become part of a different
dispatch code-generation shape.

Keeping only `channel_members_snapshot` out of line with `#[inline(never)]`
restores the native receive-batch body, now 12,109 bytes. The inspection helper
is 843 bytes and still uses the same per-room metadata checks. This keeps
inspection code from changing the compiler's hot dispatch specialization;
it adds no routing lock, allocation, scheduling threshold or changed wire
behavior. The attribution is specific to the pinned compiler and these builds,
not a promise about every compiler or machine. Symbol-size records and the
rejected first-repair measurements remain with the raw evidence.

## Timing results

The five-run pre-Claude/Claude remeasurement gave mixed fanout medians of
730,786 and 701,908 deliveries/s (−4.0%), with overlapping ranges of
544,188–853,526 and 555,540–911,774. Combined private-plus-fanout throughput
was 1,536,587 versus 1,553,209 deliveries/s (+1.1%). Isolated room fanout was
1,632,367 versus 1,706,809 (+4.6%). These results do not establish the initial
three-run 14% figure as a stable architectural regression.

The following is the final five-run matched set, comparing Claude's unchanged
binary with the selected repair. Each cell is a median of fresh runs. The
earlier five-run outlined-helper screen is retained separately and also
recovered mixed throughput.

| Workload | Claude | Final repair | Change |
| --- | ---: | ---: | ---: |
| Mixed: room fanout deliveries/s | 817,801 | 871,889 | +6.6% |
| Mixed: private + fanout deliveries/s | 1,640,560 | 1,659,561 | +1.2% |
| Isolated room fanout deliveries/s | 1,687,246 | 1,649,309 | −2.2% |
| Private-only deliveries/s | 1,280,947 | 1,285,533 | +0.4% |
| Private-only, eight workers, deliveries/s | 2,579,467 | 2,583,901 | +0.2% |

The mixed fanout ranges remain broad: 543,462–864,924 for Claude and
535,187–946,688 for the repair. Isolated ranges overlap at
1,601,511–1,725,328 and 1,626,219–1,746,521. This is evidence against retaining
the earlier repair's consistent mixed slowdown, not a universal throughput
improvement. Final fanout p99 histogram bounds remained 8.389 ms in the mixed
case and 4.194 ms in isolation. Server CPU was 2.079 occupied cores for the
mixed case, 1.635 in isolation and 2.107 for private-only traffic; RSS medians
were 7.35, 7.16 and 5.88 MiB respectively.

The final one-worker control removes cross-worker connection placement from
the comparison. Fanout medians were 221,058 versus 220,087 deliveries/s
(−0.4%); combined medians were 467,101 versus 465,138 (−0.4%). Both versions
had the same p99 bounds. The larger mixed-workload loss from the first repair
was not retained.

The eight-worker check retained the 0.524 ms private p99 bound, with 4.485
occupied server cores and 6.15 MiB RSS on the final build. The full investigation
contains 150 ordinary timing runs across the control, rejected and selected
builds; diagnostic profiles and acceptance-only fixtures are separate.

## Final acceptance results

The final source passed `cargo fmt --check`, Clippy with warnings denied,
251 Rust tests (one ignored), all 67 JavaScript tests, four new notice-checker
tests and seven existing fixture tests. The new checker tests also run in CI.

All 29 acceptance runs on the selected executable passed:

- Three runs each of full HTTP, CBOR and paged HTTP inspection with concurrent
  private traffic and room churn. The inspection client was unchanged.
- Five eight-source observer barrier runs, plus one-slot, constrained-budget,
  idle-population and burst-teardown variants.
- Channel, private, combined room/observer and concurrent-publisher observer
  message fixtures using their maintained queue settings.
- The 100,000-user capacity profile, two complete room mixed runs and two
  complete observer mixed runs, all with inspection enabled.
- Warm-cache shutdown and retirement-fallback checks.

Across the four mixed runs, all 128 offered fanout windows completed:
6,400,000 stable-user probes, 4,194,304 fanout deliveries, 4,096 churn
lifecycles and 3,200 inspections were verified. Every run passed the full
delivery, offered-load, process-exit and resource-cleanup gates.

| Selected build workload | Verified traffic | RTT p99 upper bound | Worst RTT | Server RSS |
| --- | --- | ---: | ---: | ---: |
| 100,000 users, paced capacity | 400,000 deliveries | 0.262 ms | 2.907 ms | 114.66 MiB |
| Mixed room, two runs | 1.6M stable probes + 1.049M fanout per run | 2.097–4.194 ms | 6.523 ms | 122.15–122.71 MiB |
| Mixed observers, two runs | 1.6M stable probes + 1.049M fanout per run | 4.194 ms | 5.266 ms | 127.11–127.35 MiB |

RTT in the mixed rows describes the paced stable-user probes, not fanout.
Worst fanout latency was 121.471 ms for room scope and 87.145 ms for observer
scope; the largest fanout p99 bound was 134.218 ms. Server CPU ranged from
3.923 to 4.104 occupied cores. Capacity consumed 1.93 server CPU seconds
(0.40 user, 1.53 system) over its measurement gate. All checked final capacity
resource gauges and the routing-copy counter were zero. These short runs
establish the exercised acceptance cases, not a long-term latency guarantee.

## Evidence and reproduction

The [versioned measurement summary](acceptance-fanout-2026-10-21.json) contains
individual timing samples, ranges, acceptance outcomes, capacity metrics,
source and binary hashes, and validation-log digests. Labels distinguish the
rejected first repair from the final build.

Raw JSON receipts, per-user latency CSVs, logs, source patch, frozen binaries,
exact commands and all emitted numeric fields are retained locally under
`artifacts/acceptance-fanout/`. `baseline-manifest.json` compares pre-Claude
with Claude; `fixed-manifest.json` and `confirmation-manifest.json` retain the
first repair's comparison. `outlined-manifest.json`, `final-manifest.json`
and `final-one-worker-manifest.json` measure the selected build against Claude.
`final-acceptance-manifest.json` records acceptance of the selected executable;
earlier acceptance receipts are retained separately.

The benchmark command for one mixed room trial is:

```sh
python3 scripts/inspection.py --server /absolute/path/to/frozen/server \
  --client /absolute/path/to/frozen/inspection --rooms 0 --pairs 32 \
  --mode none --inspectors 0 --churn-workers 0 --workers 4 --client-workers 8 \
  --seconds 4 --server-cpus 0-7 --client-cpus 8-15 \
  --pin-workers --pin-client-workers --fanout-readers 1024 --fanout-window 4 \
  --output artifacts/fresh-trial.json
```

Use `--pairs 0` for isolated fanout. Remove the two fanout options for
private-only traffic. Alternate the frozen server paths and use fresh receipt
names. The full mixed acceptance recipe is in
[the performance guide](../performance-guide.md); use four seconds and run
both room and observer scopes with inspection enabled.

An unprivileged CPU-profile attempt failed because the host denied `perf`
event access. Scoped privileged captures then succeeded without changing any
system profiling setting. Their largest user-space samples were session lookup,
socket send/notify, output flushing and decoding in both versions; the listing
functions were not among the reported hotspots. The Claude capture lost 125
samples; the fixed capture lost none. These profiles are supporting evidence,
not precise attribution of the throughput difference. No profiler timing is
included in the ordinary comparison. A separate extra observer probe configured one
queue slot for eight simultaneous publications and received Close `1013`, as
the pressure policy allows. The maintained concurrent fixture uses its normal
queue setting; its repeated results are separate from that pressure diagnostic.
