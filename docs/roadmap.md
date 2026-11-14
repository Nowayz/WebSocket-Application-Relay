# Roadmap

The goal is one small, maintainable relay platform: a Rust WebTransport server
and a CBOR JavaScript/TypeScript client that supports 100,000 connected users
with predictable response latency, shared payload ownership, per-recipient FIFO
and bounded retained memory. Performance claims must name the host, message
sizes, traffic mix and offered rate. Connected population alone is not a
capacity claim.

## Product boundary

The supported surface is the Rust server/library, the CBOR JavaScript client
with TypeScript declarations and browser bundle, protocol documentation, the
management API, correctness checks and short Rust load clients. WebTransport is
the only transport; earlier transports remain only in Git history. Do not
rebuild a compatibility layer. `scripts/check-product.py` audits this boundary.

## Next work

The [measured profile](performance.md) shows where a private delivery's CPU
goes. The largest remaining share is QUIC packet, acknowledgement and stream
work in user space, followed by the kernel's UDP path and encryption. In the
closed-loop workloads the relay is not CPU-bound, so round-trip latency (wake-up
chains between the ingress thread and connection tasks) limits delivery more
than CPU. Measure before changing either; several plausible changes were
rejected in paired runs.

Bounded routing work stays a requirement. Large room, global and observer
broadcasts advance through immutable recipient views with no registry lock
held, in 1,024-recipient turns; deferred departures use 128. Cold recipient-view
construction and invalidation under membership churn remain the next hold-time
target: the first global publication after a membership change still visits
every channel. Observer barriers scan pending departure jobs in chunks of at
most 128 per shard-lock hold; total scan work still grows with pending work.
Preserve message-before-unsubscribe-ACK ordering, source FIFO, cancellation, ID
takeover and output-budget refusal as this design evolves. The current locking
model and queue design are in [architecture.md](architecture.md).

Reordering-heavy paths still slow large reliable transfers (see
[performance.md](performance.md#impaired-networks)). Clients that keep QUIC's
packet-count loss rule on their own uploads are affected more than the relay.

## Acceptance for each change

Use a short focused control/candidate test before a short mixed 100,000-user
screen. Freeze source and executable identities, reuse the same client and CPU
placement, alternate run order and serialize builds and measurements. Seconds
of steady traffic can answer a narrow scheduling question; setup, complete
population checks and teardown still take additional time. Reserve longer soaks
for a specific unresolved lifecycle or retention concern.

Check the following together:

- Every connected recipient's samples, intended-send delay, worst observed
  latency and completed offered work. Report the sample count; a few packets
  per user cannot establish a production tail-latency guarantee.
- Independent private/control responsiveness during multicast, joins,
  departures, observer revocation and slow-consumer pressure.
- Exact sender, value and ordering; permission and acknowledgment boundaries;
  exact delivery/counter totals and graceful close.
- Server CPU, retained storage and queue/staging/variable reservations. Payload
  sharing should survive partial writes and fanout without a body copy for
  each recipient. This does not promise zero allocation or zero kernel copies.
- Behaviour on impaired networks (loss, jitter, reordering) for changes to the
  QUIC stack.
- Cleanup to zero live resources and reservations, with intentionally reusable
  arena capacity reported separately from logical retention.

Retain a change only when the complete workload justifies its CPU, throughput
and memory tradeoffs.

## Operational completion

Keep admission, backpressure, paginated inspection, drain/resume and shutdown
under the same acceptance checks. Make overload observable and recoverable
without unlimited queues. Consider adjacent management features only when an
operating need is measured or demonstrated. Persistence, distributed routing
and additional runtime backends are separate design decisions; they are not
prerequisites for the 100,000-session single-node milestone.
