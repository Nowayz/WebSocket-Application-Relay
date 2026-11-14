# Changelog

## Unreleased

- **WebTransport is the only transport.** The WebSocket server
  (`--legacy-websocket`), its vendored `tungstenite` and `tokio-tungstenite`
  copies, the TLS front-end fixture and every WebSocket-only test, example and
  benchmark script are removed. Clients connect to `https://HOST/war`; the
  relay terminates TLS itself with `--tls-cert`/`--tls-key` (or `--dev-cert`
  locally), so the Caddy reverse proxy is no longer part of the deployment.
  Removed options: `--legacy-websocket`, `--listen-backlog`,
  `--tcp-send-buffer-bytes`, `--tcp-receive-buffer-bytes`,
  `--read-buffer-bytes` and `--read-buffer-idle-secs`. Removed metrics: the
  routing reuse/copy, accept/setup error, decoder reclaim, aborted/cancelled
  write, close-failure, transport-disconnect, join-rejected, native slot and
  peak output-queue counters. The JavaScript client drops `WebSocketClass`
  (use `TransportClass` to inject a transport) and renames `relay.ws` to
  `relay.transport`. The systemd unit receives the certificate as systemd
  credentials and binds UDP 443.
- A refused join now closes the WebTransport session with its own code
  (`1013` for admission or drain) instead of a generic `1002`. Origin and
  per-address refusals now count in `war_rejected_total`, as documented, and an
  origin refusal no longer counts as a handshake error. Aborting the
  `webtransport::serve` task, not only cancelling it, now stops admission and
  closes every connection with `1001`.
- Workers are pinned one per CPU by default when the process may use exactly
  one CPU per worker (`--pin-workers` is now `auto`, `on` or `off`; the bare
  flag and `WAR_PIN_WORKERS=true` still mean `on`). Under a closed-loop
  workload private delivery rises 14-16% and room fanout 0-4%. QUIC stream
  reads copy from the receive assembler in place instead of splitting the
  shared packet buffer, removing two atomic updates per record; CPU per
  delivery falls 1-4%.
- The relay binary uses jemalloc as its global allocator on non-MSVC targets.
  Reliable throughput rises 2-3% for private routing, about 17% for room
  fanout and about 8% for mixed traffic. Idle QUIC connections release their
  drained endpoint-event, handshake and handshake-frame queues, start receive
  assemblers with one slot, and keep the previous path and 0-RTT secret out of
  line; ready memory at 100,000 sessions falls from about 1,839 to about
  1,671 MiB.
- The relay tolerates packet reordering. QUIC connections hold 1-RTT packets
  that overtake the end of the handshake instead of dropping them, retire a
  peer's old connection ID only after the packets sent to it are resolved, and
  the relay detects loss by the time threshold alone rather than also by
  packet count. A 1 MiB reliable transfer over a 100 ms path with 3 ms of
  jitter falls from 5.8-8.3 s to 1.36-1.47 s in every run; under 2% loss it
  takes about 1.0 s. `scripts/freshness-matrix.py` gains `--mtu`,
  `--client-packet-threshold` and a `jitter` profile.
- HTTP and CBOR channel listings omit empty or retired ordinary rooms captured
  during concurrent admission or departure; the permanent `re_globl` room
  remains visible when empty. The metadata collector stays out of line so
  the pinned release compiler preserves routing specialization.
- The routing registry no longer has a global lock. Session ownership and
  the channel index are lock-free concurrent maps, pre-sized to reduce resize
  work, each channel has its own
  lock, global observers are an atomically swapped snapshot and admission
  counters are atomics, including an exact channel count that enforces
  `WAR_MAX_CHANNELS` under concurrent creation. No output write happens
  while a registry lock is held. The welcome frame is still written before a
  session is published to its room, so it remains first in the session's FIFO.
- The global observer list is an immutable snapshot swapped atomically, with
  a generation-based barrier replacing the lock a departure used to hold from
  capture to enqueue. Flag changes and observer departures publish the next
  snapshot with a compare-and-swap loop instead of serializing on a writer
  mutex, and routing reads which observer classes exist from the snapshot it
  loads. Direct deliveries to observers take no shared lock: each recipient
  re-checks the revocation mark under its own output lock, so a frame either
  precedes the revocation's acknowledgement in that observer's FIFO or is
  dropped. The index of large observer publications is a lock-free map; a
  barrier inspects publications one job at a time. The global
  recipient-view cache and the
  disconnect-observer snapshot cache are atomic swaps, the list of departure
  queues with pending work is a lock-free queue, and session ownership is a
  lock-free map. No lock shared by the whole relay remains on the routing
  path. A publication or departure captured concurrently with a revocation
  may be omitted for that observer; it never arrives after the
  acknowledgement.
- A join that loses the race with its room's retirement after the welcome
  has gone out now retries against the replacement room under the same ID.
  It used to start over with a fresh ID and rolled the session count back
  twice, which could underflow the count and refuse later joins.
- Departure notices use one queue per room instead of one global FIFO.
  `re_globl` departures and disconnect-observer notices are spread over
  sixteen queues by the departing session's room; a revocation barrier
  scans them one at a time, at most 128 jobs per hold, with one watermark
  per queue, so nothing holds two of them and a long scan releases the shard
  between chunks. The synchronous fallback a departure takes when
  staging credit is refused drains the queue in holds of at most eight
  turns. Departure turns rotate across queues in bounded turns (128
  recipients for departures, 1,024 for live broadcasts), allowing other queues
  to receive service before a large room finishes. Notices for different rooms, including
  disconnect notices for sessions of different rooms, are no longer ordered
  relative to each other.
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
- `Config::default()` no longer reads `WAR_*` environment variables; only
  `Config::parse()` does. Embedded relays and tests are reproducible regardless
  of the calling shell.
- The base deployment profile's staging allowance drops from 1 GiB to
  256 MiB so that every budget fits under the unit's `MemoryMax=1G`, and the
  unit gains `MemoryHigh=768M`.
- New end-to-end test for a slow consumer, a unit test that a publication
  outlives its source, and two responsiveness tests: one drives a 1,100-member
  room through broadcasts, departures and observer revocations, the other
  creates and retires rooms continuously under a stream of global broadcasts
  while an admin client pages through the channel listing; in both an
  unrelated session must keep echoing within one second, every listing
  must answer within one second, and shutdown must stay bounded. Two unit
  tests race channel creation against the channel limit and joins against
  retirements of one name, a third stalls a direct observer delivery
  across a revocation and checks that it is dropped rather than delivered
  after the acknowledgement, a fourth revokes an observer with pending
  notices in two shards and checks that each arrives exactly once, a fifth
  revokes an observer behind 3,000 queued jobs in one shard and checks that
  every notice arrives exactly once, and a sixth drains 300 queued jobs
  through the synchronous fallback in order. The room-churn responsiveness
  test also runs 130 disconnect observers, so every departure is a queued
  observer job, plus one that keeps clearing and restoring its flag, so
  every clear is a chunked barrier scan racing those jobs.
- `check-release.py` uses explicit checks instead of `assert` and verifies
  `SHA256SUMS`; the operator archive no longer includes `docs/reports`.
- Removed the unreferenced `examples/encode_cost.rs`, `examples/placement.rs`
  and `scripts/plot-user-latency.py`.
- Documentation: dated measurement reports moved to `docs/reports/`; new
  `docs/architecture.md`, `docs/glossary.md`, `docs/performance.md` and a
  configuration reference in `docs/operations.md`.

## 0.1.0

Initial Rust/CBOR release candidate. See [docs/release.md](docs/release.md).
