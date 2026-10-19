# Scope departure fallback to overlapping channels

A departure normally publishes a charged, bounded retirement job. If its source
cannot fund that job, the relay uses synchronous best-effort delivery. That
fallback previously finished every pending retirement while holding the registry
write lock. A slow sender in a two-member room could therefore force submission
to 100,000 unrelated recipients before its own room received a notice. Other
registry users waited behind the same work.

Every immutable retirement view contains peers from one channel: either an
ordinary room or the global observer channel. The fallback now finishes preceding
jobs only for the affected recipient channel, preserving their queue order. It
leaves disjoint jobs in their original order with their reservations and normal
1,024-recipient turns. A global observer overlaps notices from different source
rooms, so the global fallback still finishes that observer channel's preceding
jobs before direct delivery.

The classifier reads the first retained peer's immutable channel. It requires no
new per-peer field, recipient index, allocation, payload copy or lock. It scans
pending job descriptors rather than visiting unrelated recipients. Recreated
rooms with equal channel bytes are conservatively grouped; captured peer owners
still prevent redirecting old work to a replacement session. Existing global
credit, slow-recipient policy, body sharing and sequence watermarks remain in
force. Synchronous work for the overlapping channel can still be large, and a
long job queue still takes a linear descriptor scan.

## Native pressure screen

`scripts/retirement-fallback.py` creates a large room with one departure source
and 100,000 healthy audience members, a separate two-member room and an
independent echo witness. Only the small-room source uses a tiny receive window.
A full-width assigned ID and a 65,536-byte encoded self-echo retain exactly
65,536 bytes in that source's 65,546-byte output allowance. The fixture confirms
that charge through management metrics. Shared queue credit remains 256 MiB.

After the large-room source closes normally, the fixture administratively
disconnects the blocked small-room source. That source cannot fund a 19-byte
departure body from its remaining local allowance, forcing the real fallback.
The admin operation deliberately cancels its partial echo. Exactly one aborted
write is required for that source; other error counters must remain unchanged.
The fixture does not treat that administrative cancellation as a healthy-client
Close handshake.

The receipt verifies the small-room notice, every large-room notice, zero residual
queued credit, zero recorded routing-copy bytes, expected physical/session counts,
and all 100,000 audience shutdown Close `1001` frames followed by TCP EOF. Servers
exit normally without a forced kill. The existing terminal policy does not wait
for peer Close acknowledgments.

The same final fixture ran serially control/candidate/candidate/control, using
uninstrumented frozen executables, eight server workers on CPUs 0–7 and clients
on 8–15,24–31. These are loopback observations including management HTTP and
client/kernel scheduling, rather than isolated relay-hop measurements.

| Run | Admin request through small-room notice, ms | Worst independent witness RTT, ms |
| --- | ---: | ---: |
| Control 1 | 56.416 | 53.337 |
| Candidate 1 | 1.411 | 0.695 |
| Candidate 2 | 3.613 | 0.832 |
| Control 2 | 108.102 | 53.577 |

All four completed the full correctness/resource matrix. Population setup took
about four seconds per run; the event itself was short. Two samples of each
implementation do not establish a sustained percentile or an all-user latency
SLO. One witness diagnoses the cross-room stall. The focused regression verifies
that no unrelated room recipients are visited during fallback and that an
overlapping global observer receives both notices in order. Existing same-room
budget-refusal, sequence exhaustion and observer-barrier regressions also pass.

An earlier control repeat failed the fixture's post-event witness-coverage check
before verifying the audience. A fixed 20 ms sampling sleep was insufficient to
guarantee a post-event request. The fixture now waits for an observed post-event
reply with a five-second deadline. That failed receipt and the earlier exploratory
fixture remain preserved; neither supplies a row in the accepted comparison.

## Ordinary mixed-load checks

Four two-second steady phases used 100,000 capacity users, 8,192 room readers,
32 private pairs, two inspectors, eight paced churn workers and eight offered
fanout windows per second. Every run completed all 16 windows, 524,288 multicast
deliveries, 512 churn cycles and 800,000 capacity observations. Each user supplied
eight observations, so the CSV's reported per-user p99 equals that user's observed
maximum; it is not a stable tail estimate. All exits were `[0,0,0]`, final resource
gauges were zero and no routing-copy bytes or pressure/error violations appeared.

| Run | Server CPU, seconds | Median user's observed maximum, ms | Worst observed user, ms | Private deliveries/s | Largest fanout window, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.36 | 0.959 | 6.369 | 1,314,119 | 60.836 |
| Candidate 1 | 9.28 | 1.145 | 7.688 | 1,019,449 | 107.134 |
| Candidate 2 | 9.35 | 1.103 | 5.041 | 1,267,703 | 69.305 |
| Control 2 | 9.36 | 1.171 | 5.605 | 1,052,751 | 79.503 |

The first candidate's ordinary latency/throughput was worse than the first
control's. Reversing the order produced a candidate with better observed user
maxima and private throughput than the second control. These short results vary;
they establish neither a general forwarding improvement nor a sustained latency
SLO. The change is retained for its repeatable cross-room fallback benefit, with
ordinary correctness, offered-load and bounded-resource gates intact.

## Verification

All 214 Rust tests and 66 JavaScript tests passed, with formatting, Clippy warnings
denied, vendor provenance and the maintained-product audit. The new two-worker,
2,048-audience native CI fixture passed its notice/Close/EOF and resource checks.
The JavaScript fixture retired all six checked resource gauges to zero.

The separate constrained-credit observer screen passed with 100,000 global
observers, eight departures and a 2 MiB queue budget: 1,100,009 expected notices,
ordered unsubscribe acknowledgment, re-subscription, zero pressure/copy counters,
zero residual queued credit and 100,000 shutdown Close receipts. Its unsubscribe
ACK took 1.220 ms and its worst independent witness RTT was 1.730 ms. These checks
exercise overlapping global delivery as well as the optimized disjoint-room case.

## Reproduction

```sh
taskset -c 8-15,24-31 python3 scripts/retirement-fallback.py \
  --server /absolute/path/to/frozen/war-relay --server-cpus 0-7 \
  --audience 100000 --output artifacts/retirement-fallback.json
```

Adjust masks to the host. CI uses 2,048 audience members and two workers without
fixed masks. Files refuse reuse. Receipts include executable/fixture/helper hashes,
full commands, staged and final metrics, monotonic event timestamps, witness
samples, notice/Close/EOF counts, process information and retained error logs.
The script uses a known disposable fixture password/token, never deployment
credentials.

Control: Rust commit `13fb676`, executable SHA-256
`002aaaeee203a9e29d3231eef1a09dae65da43df53704decae486ed9908f5bdd`.
Selected candidate executable SHA-256:
`35dae946d6de535b8edcd038411bcc33fc46000b8cdd1ad1d608c4f0db9a7cd6`.
The candidate's copied source, build receipt and binary are preserved alongside
the event and mixed-load receipts. Short accepted event timings justify scoping
the barrier; they do not complete the ongoing 100,000-user reliability goal.
