# Shared, charged disconnect snapshots

Overlapping departures to an unchanged disconnect-observer population used to
copy the entire recipient list for each job. With 100,000 observers, eight
pending jobs could retain about 6.4 MB of handle arrays. The observer-specific
barrier already avoided writing unrelated recipients, but publication still
cloned and later dropped those repeated arrays under shared synchronization.

The first candidate reused the list through a weak registry cache. That reduced
copying, but conservatively charged the same shared array once per job. At a
2 MiB shared queue limit, both the baseline and this cache-only candidate failed
the complete notification matrix under queue pressure. Sharing the storage
without sharing its reservation did not fix that operating case.

## Ownership and accounting

A `RetirementSnapshot` now owns the immutable observer vector and its global
queue reservation. Pending disconnect jobs share that owner and independently
reserve their own descriptor/body bytes. The registry keeps only a weak cache;
changing disconnect membership invalidates it, while jobs keep their original
view. Enabling another observation class leaves disconnect membership reusable.

The last owning job frees the recipient buffer before releasing its view credit.
The weak cache retains no recipient handles, payloads or sockets after the jobs
drain. It can retain one small expired control block until the next capture,
membership change or shutdown. Different retained membership generations each
carry their own reservation. A view that cannot fit still takes the existing
best-effort fallback; the limit has not been raised or bypassed.

`FanoutRecipients` distinguishes ordinary room views from funded disconnect
snapshots. Ordinary room/publisher accounting remains conservative per job.
FIFO publication, sequence watermarks, recipient-owner output, payload sharing,
permission boundaries and slow-recipient policy retain their contracts. The
change adds no lock and no per-peer field. On this host `Peer` remains 88 bytes;
`RoomFanout` grows from 112 to 120 bytes per allocated continuation/job. A shared
snapshot has a 40-byte owner plus the Arc header and handle buffer; the weak
registry cache is eight bytes. Active retirement descriptor/view storage is
reserved against shared queue credit. These sizes do not predict whole-process
RSS or allocator overhead.

## Short burst measurements

The control is Rust commit `28e97de`. Each checked event uses 100,000 audience
observers, eight distinct singleton departure sources, the tested observer and
one independent echo witness. Sources close sequentially before revocation.
The complete matrix verifies 1,100,009 notices, their order, re-subscription,
zero residual queued credit, physical/session counts and 100,000 shutdown Close
`1001` receipts. Every accepted process exits normally without a forced kill.

| Run | First source Close through unsubscribe ACK, ms | Unsubscribe ACK alone, ms | Worst witness RTT during the short event screen, ms |
| --- | ---: | ---: | ---: |
| Initial control | 19.716 | 3.617 | 5.325 |
| Later control | 21.710 | 1.290 | 7.283 |
| Selected candidate 1 | 15.212 | 1.740 | 2.940 |
| Selected candidate 2 | 7.970 | 1.891 | 2.119 |

Median close-burst-through-acknowledgment time fell from 20.713 to 11.591 ms,
about 44% in this finite screen. ACK-only ranges overlap; this does not establish
a separate universal ACK improvement. Timings include client/kernel scheduling.
One witness is not an all-user latency measurement or a long-term p99 guarantee.
The intermediate cache-only candidate also passed normal bursts at 7.575–14.594
ms but retained the pressure failure; it is not the selected implementation.

### The same 2 MiB credit limit

| Implementation | Complete eight-departure matrix | Global queue-pressure counter at failure or completion |
| --- | --- | ---: |
| Control | Failed | 41,242 |
| Cache-only candidate | Failed | 34,107 |
| Shared-owner candidate, first run | Passed | 0 |
| Shared-owner candidate, repeat | Passed | 0 |

Both selected runs checked all 1,100,009 notices, all 100,000 shutdown Close
receipts and zero recorded routing-copy bytes. Their close-burst-through-ACK
intervals were 9.716 and 11.636 ms. Queue credit remained exactly 2,097,152 bytes;
the fixture uses a 64 KiB message ceiling and the same fixed small encoded values.
Pressure-counter values are refused reservations, not exact lost-message counts.
The failed matrices are retained as failures and supply no accepted latency row.

With the ordinary queue limit, sampled retained credit just after the ACK was
about 6.44 MB for the controls and 0.83 MB for the selected candidates. These
samples include other queued work; they are not isolated allocation counts.
This removes duplicate view charges and repeated arrays rather than granting
unbounded credit or weakening the delivery checks.

## Normal mixed-load screen

Four serial control/candidate/candidate/control runs used 100,000 capacity users,
8,192 room readers, 32 private pairs, two inspectors, eight churn workers at
32 cycles/s and eight offered four-message fanout windows/s. Steady phases were
two seconds; population setup and exhaustive verification add wall time.

| Run | Server CPU, s | Private deliveries/s | Median user's observed maximum, ms | Worst user, ms | Worst fanout window, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control 1 | 9.27 | 1,254,672 | 0.982 | 5.399 | 95.231 |
| Candidate 2 | 9.28 | 1,050,946 | 1.078 | 4.752 | 76.843 |
| Candidate 3 | 9.25 | 1,053,527 | 1.120 | 4.779 | 61.897 |
| Control 4 | 9.36 | 1,061,947 | 1.179 | 5.452 | 73.969 |

All runs completed 16/16 offered windows, 524,288 multicast deliveries, 512 churn
cycles and 800,000 capacity observations. Every user supplied eight samples;
its reported p99 therefore equals its observed maximum. All process exits were
`[0,0,0]`, and the checked resources retired. Worst intended-send latency ranged
from 6.872–7.234 ms for controls and 5.893–7.191 ms for candidates.

CPU was essentially unchanged. Private throughput was below the fastest control,
so this is not selected as a general throughput improvement. These short samples
cannot distinguish all placement/scheduling variation or certify a sustained
fairness SLO. Selection rests on the repeatable burst improvement and the
previously failing fixed-credit matrix passing with preserved semantics.

## Verification and remaining limits

All 211 Rust tests and 66 JavaScript tests passed, with formatting, Clippy warnings
denied, vendor provenance and the maintained-product audit. New regressions check
view reuse, role/membership invalidation, late observers, first/last owner release,
multiple jobs fitting one view's fixed credit, and refusal when a view cannot fit.
Small native CI checks cover a ready one-slot ACK and eight departures under
65,546 bytes of credit with 2,048 observers.

A first capture and final handle release remain linear in the observer population.
The revocation barrier still scans queued global recipient views. Insufficient
credit for a full view or several distinct retained generations can still trigger
synchronous fallback and best-effort losses. Those are further profiling targets;
this change does not promise delivery under every undersized budget.

Reproduce on the measured host using separate server/client physical cores and
fresh output names:

```sh
taskset -c 8-15,24-31 python3 scripts/observer-barrier.py \
  --server /absolute/path/to/frozen/war-relay --server-cpus 0-7 \
  --audience 100000 --departures 8 --output artifacts/departure-burst.json
```

Add `--queue-budget-bytes 2097152` for the constrained-credit comparison. This
allows the pressure counter to change but retains the complete wire matrix,
other error gates and cleanup checks. `queue_pressure_observed` reports whether
any refusal occurred; the selected implementation avoids those refusals in this
case. The [performance guide](performance-guide.md) has the matched mixed command.

Frozen server SHA-256 identities are control
`1668b00176a4b9d5cc1610128108c8e335636f79471afb7b8497d5d0b7b39738`
and selected candidate
`c87aed67d7b4e2c077c6fa025f2b1f8bcbd799edcf118fcadbdfcd13473c5c44`.
Source/build digests and complete local receipts accompany the selected binary.
The initial fixture predates the post-verification diagnostic pressure-counter
field; later revisions improve diagnostics and physical-count checks. These
changes do not alter the encoded workload or the timed close/ACK path. Exact
fixture versions are retained with their receipt hashes.
