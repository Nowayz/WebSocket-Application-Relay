# Close lifecycle, sustained traffic, and backpressure

## Why these checks were added

The first finite load checks established basic routing and memory behavior. A
longer real-socket workload was needed to exercise repeated close handshakes,
heartbeat traffic, unread recipients, and cleanup under ongoing delivery.

`examples/stress.rs` uses fixed traffic templates and a fixed 64-bucket logarithmic
latency histogram, so retained workload memory is independent of run duration.
Every expected frame at a healthy recipient is compared with its bytes and order. Its 64 distinct
cyclic templates exceed the maximum outstanding window of 32: the next window
does not begin until all current expected frames arrive. Churn instead uses a
monotonic eight-byte counter per worker, verifies its self-message against the
new session ID, and completes a close handshake before reconnecting.

The runner starts a fresh four-worker server for each case and samples server RSS,
CPU, HTTP response time, and relay counters every 250 ms. Cases run sequentially,
without concurrent compilation. It verifies cleanup to zero connections, sessions,
queued bytes, and variable bytes, with only the persistent global channel left.
The current runner also cross-checks verified messages and joins against the
server's independent counters and requires exactly one slow recipient to be evicted.

## Two failures reproduced before the fix

**Normal peer close was not acknowledged.** Tungstenite queues a Close reply when
it reads a peer Close. The original Rust path called `WebSocketStream::close`
again, which attempts a fresh write after closing has started and fails with
`SendAfterClosing`. The queued acknowledgement was not flushed before TCP dropped.
The 64-worker churn case therefore failed in its first wave with
`ResetWithoutClosingHandshake`. A focused Rust regression also failed with that
exact error before the fix. This lifecycle follows
[tungstenite's documented read/close contract](https://docs.rs/tungstenite/0.30.0/tungstenite/protocol/struct.WebSocket.html#method.read).

**A pending write ignored session cancellation.** The unread-recipient case
overflowed its output budget by the 0.276 s sample. Its three-session registry and
roughly 0.9–1.0 MiB of queued output remained until the 10.058 s sample. The write
waited for its five-second deadline, then another close/flush attempt waited for
a second deadline. Healthy delivery continued, but failed-session capacity and
output reservations remained charged for about 9.8 seconds after detection.

The fixes:

- Flush the automatically queued acknowledgement for peer-initiated Close,
  preserving its valid code/reason. Share this lifecycle with the legacy bridge,
  including connections that have not joined yet.
- Use one deadline for an initiated close send plus acknowledgement. Remove
  routing membership and release queued reservations before this handshake.
- Interrupt output on the session close signal or shutdown. If an output future
  is cancelled, times out, or fails, drop its transport; do not retry a potentially
  partial frame through another blocking flush.
- Separate transport loss, write timeout/error, cancelled output, and failed
  close-handshake diagnostics from malformed WAR/CBOR packets.

## Before/after observations

The baseline server was commit `1b975de`; after records contain the close fixes.
Both sides used the same workload behavior: Linux loopback, four server/client
workers, rates disabled for stress, heartbeats enabled, a 1 MiB incoming limit,
and a queue budget of 1 MiB + ten bytes. Binary hashes are retained in the raw
records. These are one run per case, not repeated capacity estimates.
The after records were captured with the fixes still uncommitted: their `revision`
field identifies the base checkout, while their server binary hash identifies
the measured build. New records also include working-tree state and source hashes.

| Case | Before | After |
| --- | --- | --- |
| 64 clients, private 1,024-byte payloads, 30 s | 37,750,096 verified deliveries | 35,328,176 verified deliveries |
| Sustained throughput | 1,258,301 deliveries/s | 1,177,575 deliveries/s |
| Sustained p99 histogram upper bound | 1.049 ms | 1.049 ms |
| Peak server RSS in traffic case | 6.69 MiB | 6.73 MiB |
| 64 reconnect workers, up to 20 s | Failed after first wave: missing Close reply | 1,911,994 complete connection lifecycles |
| Reconnect protocol/close failures | Failed workload | Zero |
| Unread recipient, 12 s, 65,536-byte payloads | Gone at 10.058 s sample | Gone at 0.276 s sample |
| Healthy deliveries during unread-client case | 565,437, all verified | 530,408, all verified |
| Maximum sampled management HTTP latency, traffic case | 2.76 ms | 2.72 ms |

The roughly 6.4% sustained-throughput difference is observed, not attributed.
Output cancellation adds work, but the comparison has only one unpinned run per
side; repeats and profiling are needed to distinguish implementation cost from
variation. The stable p99 histogram bucket is an upper bound, not a precise
quantile. No throughput gain is claimed for these reliability fixes.

The unread recipient was detected and removed within the same 250 ms sample after
the fix, so the trace does not resolve its exact eviction latency. A bounded-stream
unit check separately proves a blocked write responds to policy cancellation
within its 200 ms test timeout, instead of its configured 30-second write deadline.
The healthy socket received every expected frame throughout both pressure cases.
Its maximum observed latency in that pressure case was 2.04 ms after the fix; no forced close hit it.

The unread socket eventually reports a transport reset in both versions. A Close
frame cannot be promised through a transport that is blocked mid-frame. The fixed
path releases resources promptly; cooperative native/legacy clients separately
verify clean Close acknowledgements with their exact codes and reasons.

Churn completed 1.91 million **cumulative lifecycles with 64 workers**, not millions
of concurrent connections. Server joined/incoming counters matched the client's
join and verified-message counts exactly. Server RSS ranged from 5.96 to 7.11 MiB
in the after churn samples. Thirty-second/20-second evidence does not prove
multi-hour memory stability or production capacity. The traffic/pressure clients
deliberately drop their completed sockets at teardown; resulting transport or
write-error counters are expected and no longer misclassified as protocol errors.

Raw records:

- [Baseline, including failed churn](benchmarks/2026-10-15-stability-baseline.json)
- [After close/cancellation fixes](benchmarks/2026-10-15-stability-after.json)

Metadata correction: the first raw records carry the CLI's generic
`payload_bytes: 1024` and cyclic-template description for the churn case. Churn
actually sends an eight-byte counter per lifecycle, as described above. The
current reporter labels this correctly; historical outputs are retained unchanged.

## Reproduce

```sh
cargo build --release --bins --example stress
python3 scripts/stability.py --output artifacts/stability.json
# Faster acceptance of the runner and current server:
python3 scripts/stability.py --traffic-secs 2 --churn-secs 2 --slow-secs 2 \
  --clients 8 --output artifacts/stability-smoke.json
```

The finite `load` tool bounds prepared payloads/vectors/latency samples to 512 MiB
and ten million samples. The stress tool caps its prepared templates at 512 MiB
and reuses them with a bounded histogram. These guards cover workload preparation,
not the entire client process, WebSocket buffers, or kernel socket memory.
