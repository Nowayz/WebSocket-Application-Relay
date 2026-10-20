# Rust and native CBOR roadmap

The goal is one small, maintainable relay platform: a Rust server and a native
CBOR JavaScript/TypeScript client that supports 100,000 connected users with
predictable response latency, shared payload ownership, per-recipient FIFO and
bounded retained memory. Performance claims must name the host, message sizes,
traffic mix and offered rate. Connected population alone is not a capacity claim.

## Product boundary

The [consolidation](reports/consolidation.md) removed the C/C++ implementations, temporary
native comparison dependencies, Binary/Text clients, compatibility bridge and
their dedicated build and benchmark machinery from the maintained tree. Those
wire formats are retired; CBOR strings and byte arrays are ordinary typed values
in the same protocol. The migration intentionally breaks old wire compatibility.
Keep attribution and useful benchmark findings in concise documentation and Git
history. Do not rebuild a compatibility layer or revive the comparison servers.

The supported surface includes the Rust server/library, CBOR JavaScript client
with TypeScript declarations and browser bundle, protocol documentation,
management API, native correctness checks and short Rust load clients. Audit
this boundary in CI with `scripts/check-product.py`.

## Immediate milestone: stable release

Freeze performance experiments and complete the release checklist in
[release.md](release.md). Ship a traceable server archive, source archive and
installable CBOR client package, verify the packaged server/client together, and
record the supported platform, upgrade break and operating limits. Resume the
performance work below after this release baseline is established.

## Subsequent performance work

Finish bounding routing work that can monopolize a worker or a channel lock.
Large native room, global and observer broadcasts advance through immutable
recipient views with no registry lock held. Native live fanout uses
1,024-recipient turns; deferred departures use 128. The
[global broadcast report](reports/global-broadcasts.md) records the latest measured win.
The current locking model and queue design are in [architecture.md](architecture.md).

The [native observer-message screen](reports/observer-messages.md) established membership
and independent-witness stalls at 100,000 observers. Its
[bounded-publication follow-up](reports/observer-publications.md) captures eligibility,
registers pending work for revocation and keeps one charged body across room and
observer segments. Preserve message-before-unsubscribe-ACK ordering, source FIFO,
cancellation, ID takeover and output-budget refusal as this design evolves.

Also measure cold recipient-view construction and invalidation under membership
churn. The first global publication after a membership/ownership change still
visits every channel to rebuild the global view. Do not replace the
registry or add an index merely because it might be faster; establish which
remaining lock hold, scan or allocation limits the complete workload first.
Observer barriers still scan the observer queue's pending jobs under the
listener lock. The [concurrent-publisher follow-up](reports/concurrent-observers.md) rejected
a two-entry scan cache: a large component win did not produce a clear native
eight-worker improvement. The [worker-scheduling follow-up](reports/worker-scheduling.md)
now separates fresh readiness from buffered work and bounds aggregate routing
work in both phases. Its short native and mixed-population checks preserve the
source cutoff, FIFO and storage contracts while improving responsiveness.
Failed initial routing attempts now [charge their visited handles](reports/rejection-accounting.md)
to the aggregate allowance. Changing/cold captured
generations under churn are the next lock-hold target. Do not add a recipient
index from the earlier component result alone.

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
  exact delivery/counter totals and normal Close plus transport EOF.
- Server CPU, retained storage and queue/decoder/variable reservations. Payload
  sharing should survive partial writes and fanout without a body copy for
  each recipient. This does not promise zero allocation or zero kernel copies.
- Cleanup to zero live resources and reservations, with intentionally reusable
  arena capacity reported separately from logical retention.

Choose latency objectives for explicit supported operating profiles using these
measurements. The present evidence supports individual improvements, not an
unconditional 100,000-user latency service-level guarantee. Retain a change only
when the complete workload justifies its CPU, throughput and memory tradeoffs.

## Operational completion

Keep admission, backpressure, paginated inspection, drain/resume and shutdown
under the same acceptance checks. Make overload observable and recoverable
without unlimited queues. Finish packaging and deployment examples around this
single supported stack, including the client browser artifact and migration
notes. Preserve the small protocol and management surface as the code improves.

Consider adjacent management features only when an operating need is measured
or demonstrated. Persistence, distributed routing and additional runtime
backends are separate design decisions; they are not prerequisites for this
100,000-connection single-node milestone.
