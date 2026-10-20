# Changelog

## Unreleased

- The routing registry no longer has a global lock. Session ownership is a
  64-way sharded map, each channel has its own lock, global observers have a
  separate lock and admission counters are atomics. No socket write happens
  while a registry lock is held. The welcome frame is still written before a
  session is published to its room, so it remains first in the session's FIFO.
- The global observer list is an immutable snapshot swapped atomically, with
  a generation-based barrier replacing the lock a departure used to hold from
  capture to enqueue; routing never waits on observer changes. The global
  recipient-view cache and the disconnect-observer snapshot cache are
  wait-free atomic swaps, and the list of departure queues with pending work
  is a lock-free queue. Apart from the 64 session-ownership shards, no
  process-wide lock remains on the routing path. A publication or departure
  captured concurrently with a revocation may be omitted for that observer;
  it never arrives after the acknowledgement.
- Departure notices use one queue per room instead of one global FIFO.
  `re_globl` departures and disconnect-observer notices share a queue. Workers
  rotate across queues in bounded turns (128 recipients for departures, 1,024
  for live broadcasts), so a large room's notices cannot delay another room's.
  Notices for different rooms are no longer ordered relative to each other.
- A session that closes with an unfinished broadcast (more than 1,024
  recipients) no longer truncates it. The remaining recipients receive the
  message through the departure queue, ahead of that session's departure
  notice. Previously the rest of the room silently never got the message.
- Global output pressure no longer closes the sender. When
  `WAR_TOTAL_QUEUE_BYTES` is exhausted, the frame for that recipient is dropped
  and `war_global_pressure_total` increments; the recipient is closed with
  `1013` only if it already holds at least a quarter of its own
  `WAR_QUEUE_BYTES` or the dropped frame was a reply to its own command.
  Per-session exhaustion still closes that recipient with `1013`.
- A large broadcast that cannot reserve staging credit for its recipient view
  is delivered synchronously in one longer turn instead of being refused.
- Close code `1013` now means exactly: join refused by admission or drain,
  channel variable capacity, the session's own command reply could not be
  funded, or the session is a slow consumer. See docs/protocol.md.
- Workers poll with a one-second upper bound (`POLL_TICK`), so a missed wakeup
  delays work by at most one tick instead of stalling a worker.
- On non-Linux hosts only the first worker listens and hands accepted streams
  to the other workers. Sharing one listening socket between every worker's
  poller left workers stalled at random on Windows: shutdowns ran to their
  full grace period and queued output was not flushed. Linux keeps one
  `SO_REUSEPORT` listener per worker.
- A handshake refused for lack of decoder staging credit now sends a FIN after
  its HTTP 503, so the reply reaches the peer instead of being reset.
- `Config::default()` no longer reads `WAR_*` environment variables; only
  `Config::parse()` does. Embedded relays and tests are reproducible regardless
  of the calling shell.
- The base deployment profile's decoder staging allowance drops from 1 GiB to
  256 MiB so that every budget fits under the unit's `MemoryMax=1G`, and the
  unit gains `MemoryHigh=768M`.
- New end-to-end test for a slow consumer, a unit test that a publication
  outlives its source, and a responsiveness test that drives a 1,100-member
  room through broadcasts, departures and observer revocations while an
  unrelated session must keep echoing within one second and shutdown must
  stay bounded.
- CI is split into a fast `verify` job and a scheduled `fixtures` job, with a
  cargo cache, `cargo audit`, Dependabot and a pinned `rust-toolchain.toml`.
  `check-release.py` uses explicit checks instead of `assert` and verifies
  `SHA256SUMS`; the operator archive no longer includes `docs/reports`.
- Removed the unreferenced `examples/encode_cost.rs`, `examples/placement.rs`
  and `scripts/plot-user-latency.py`.
- Documentation: dated measurement reports moved to `docs/reports/`; new
  `docs/architecture.md`, `docs/glossary.md`, a configuration reference in
  `docs/operations.md`, and an old-to-new API table in `docs/migration.md`.

## 0.1.0

Initial Rust/CBOR release candidate. See [docs/release.md](docs/release.md).
