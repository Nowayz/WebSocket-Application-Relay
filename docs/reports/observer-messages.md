# Observer message scheduling baseline

At this baseline, channel/private message observers received synchronous fanout
while the publisher held the registry read lock. The native worker yielded after
the complete message, so a large audience could hold up membership writers and
other connections owned by that worker. Other workers also had to process the resulting
output-ready backlog. The [bounded-publication follow-up](observer-publications.md)
records the implementation that addresses this measured problem.

## Short native screen on 2026-10-19

`scripts/observer-message-fairness.py` authenticates a global observer audience
and publishes distinct 1 KiB values from an ordinary room. Each round waits for
one full audience delivery, then concurrently starts an unrelated singleton
join/Close and revokes the final observer's message flag. That observer must
receive the publication before the unsubscribe acknowledgment. It immediately
re-subscribes before the rest of the audience is drained; replay, missing data
and later-window ordering errors fail the test. Every observer's exact encoded
message and sender are checked.

The first delivery is selected with Linux epoll across all audience sockets,
requesting one event. SO_REUSEPORT assigns connections independently of join
order; choosing a fixed reader can accidentally choose the publishing worker
and delay the action until its full loop ends. The fixture records both first
delivery delay and actual request timestamps. First delivery does not prove that
a continuation remains pending; deterministic partial-cursor unit checks will
also be required when introducing observer continuations.

Channel observers use eight independent private-echo witnesses. Private observers
use eight authentication-reply witnesses: a private echo would itself publish
another audience-wide message. Witnesses have one outstanding request and wait
1 ms after its reply. They are independent connections; the fixture does not
claim one witness per worker. RTT includes Python scheduling, kernel and client
processing. An overlapping sample intersects the publish-to-unsubscribe-ACK
interval and may complete afterward.

Four serialized release runs used channel/private/private/channel order, each
with 100,000 observers, ten source/target/witness sessions and two publications.
The host was an AMD Ryzen AI MAX+ 395 with 16 cores/32 logical CPUs. Eight native
workers were pinned to CPUs 0–7; the generator used CPUs 8–15,24–31. Setup took
approximately seven seconds per run. The following ranges cover the four rounds
per observer kind, not a population p99 or a service-level guarantee:

| Measure | Channel observers | Private observers |
| --- | --- | --- |
| Publication to first complete audience delivery | 0.08–4.50 ms | 0.07–34.53 ms |
| Unsubscribe request to acknowledgment | 26.52–93.58 ms | 29.01–49.44 ms |
| Independent join request to welcome | 27.10–93.74 ms | 29.15–56.14 ms |
| Worst overlapping witness RTT per round | 77.64–93.58 ms | 89.02–111.27 ms |

All four runs verified every publication, immediate re-subscription, exact
message/byte/counter totals, zero copied routing value bytes, zero queued and
variable bytes at quiescence, bounded decoder staging, and no error counters.
Source, target, witnesses and transient join peers verified normal Close/EOF.
Each remaining audience of 100,000 verified shutdown Close `1001` and EOF.
Audience peers remain live at the pre-shutdown metrics gate; the fixture does
not claim to read zero connection gauges after process exit. Terminal shutdown
avoids an artificial quadratic global-room departure storm.

A separate four-publication server-only `perf` diagnostic collected 382
userspace cycle samples with zero lost samples. The largest individual self
hotspot was `Peer::enqueue_fanout` on the publisher worker (21.19% of weighted
cycles); that worker's native `send_fanout` and `Socket::notify` accounted for
5.42% and 4.27%. These coarse samples identify the submission path as worthwhile
work; they do not measure a lock hold duration or establish instruction costs.
The profile used the earlier fixed-reader fixture and is excluded from the
ordinary timing table. Its exact source generation is preserved.

Server SHA-256:
`538c6632140e8b9916de7e7756781d7c16e8702fe16e22ffe8542acfc6b6c6e5`.
The raw artifacts are retained by the maintainer outside the repository.
`baseline-summary.json` identifies the four accepted ordinary receipts. Earlier
fixed-reader runs remain development evidence and are excluded from that table.
An intentionally failing profiler check is also retained; failure was rejected,
all six fixture peers verified shutdown, and the owned server was reaped.

## Reproduce and change safely

```sh
python3 scripts/observer-message-fairness.py \
  --server /absolute/path/to/frozen/war-relay --audience 2048 --workers 2 \
  --queue-messages 1 --kind channel --output artifacts/observer-channel.json
```

Repeat with `--kind private` and a fresh output. Both one-slot cases run in CI
and passed locally against the debug server. For the measured population use
100,000 observers, eight workers, `--server-cpus 0-7`, no explicit queue pressure,
and pin the generator to the separate client CPUs above. `--profile` is a
separate diagnostic run and reserves a fresh sibling `.perf.data` file.

The implementation target is bounded publication with captured eligibility and
discoverable pending work. Revocation must submit only that observer's older
messages before its acknowledgment, then prevent replay after re-subscription.
Source FIFO, ID takeover, cancellation, queue refusal and payload sharing must
survive the change. Merely copying the listener vector and releasing the lock
would allow an old message to arrive after an unsubscribe acknowledgment.
No production routing change or new speedup is claimed by this baseline.
