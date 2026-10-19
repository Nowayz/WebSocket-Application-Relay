# Overload retirement and admission reuse

Selected: a short standalone Linux loopback probe, scripts/overload-reuse.py. It checks that unread-recipient overload releases both admission and output resources while a separate healthy witness continues routing, then proves same-IP replacement routing. No relay runtime change is needed in the tested case. The probe reuses the existing admission/raw-CBOR client helpers rather than adding a client library or runtime mode.

Two fresh four-worker native processes each run three waves with max-connections 3, max-connections-per-ip 1, a 65,546-byte session queue budget, queue count 2, 262,144-byte global output budget and 4 KiB requested TCP send buffer. Rates and heartbeats are disabled for the fixture. Peers bind distinct 127.0.0.x addresses; no host sysctl or machine-wide resource limit is changed. There is no CPU pinning or performance comparison.

Each wave independently proves:

- A duplicate witness-IP transport is refused while only two global slots are occupied. The IP-specific counter increases, proving quota enforcement rather than inferring it from configuration.
- An unread recipient occupies the third slot. A new source IP is then refused, proving the global cap is full.
- The publisher sends bounded 32 KiB values to that unread recipient, checking a witness self-delivery between submissions. The first sampled queue charge is 32,781 bytes; the next submission triggers overload in these runs. Two encoded routed frames require 65,562 bytes, exceeding the 65,546-byte fixture budget while the first partial write remains charged.
- A publisher self-delivery acts as a FIFO input barrier before reuse. Membership retires, queued output returns to zero, and the unread transport ends. Any complete buffered frames are checked against the publisher and sent sequence. A partial frame cannot be followed by a WebSocket Close; EOF/reset is accepted for intentional overload cancellation.
- A new transport from the same recipient IP joins and self-routes, proving both its global slot and IP lease are available again. The healthy witness self-routes again. Normal Close/EOF ends the replacement.

Both recorded runs need two pressure packets/wave: six pressure messages and 15 checked control deliveries each. Healthy witness traffic occurs between each pressure submission, rather than only before and after the whole campaign. No complete buffered pressure frame is received in these runs, so this evidence does not establish FIFO or successful delivery for the overloaded recipient's discarded/partial output. Healthy control contents and sender IDs are checked; intentional lost pressure output is not treated as healthy-user loss.

| Result per fresh process | Both repeats |
| --- | --- |
| Completed overload/replacement waves | 3 |
| IP / global refusals | 3 / 3 |
| Accepted TCP / joined / retired sessions | 14 / 8 / 8 |
| Verified incoming messages / encoded bytes | 21 / 196,866 |
| Final native descriptor count / initial | 26 / 26 |
| Final sessions, connections, queues, variables, output slots, read storage | 0 |
| Native exit status | 0 |
| Observed total elapsed time | 20.5–21.0 ms |

Elapsed time includes startup and shutdown and is recorded as regression-test duration, not a connection-rate benchmark or latency bound. Each wave permits at most 64 pressure submissions (about 2 MiB) with three-second failure deadlines around readiness/retirement; the finite input bound is over thirty times the session queue budget. These are fixture guards, not new platform limits. The test uses only three simultaneous users and proves neither 100,000-user scalability nor long-term per-user latency.

All final resource gauges are zero with the root channel retained. The final descriptor census runs before server termination, preventing process exit from masking a descriptor leak. Incoming message and byte totals reconcile exactly. Each run records three aborted partial writes, as expected for the three unread peers; no protocol, handshake, rate, write-timeout or global-budget-pressure errors occur. The recorded routing-copy counter is zero in both runs. The server binary hash and all imported helper hashes are pinned in receipts; no source overlays are used to rebuild it.

The existing evidence-preservation/failure-cleanup test now runs for both admission-reuse.py and overload-reuse.py through a small inherited test class. Four tests pass in 0.592 seconds, covering existing/dangling receipt and companion-log collisions before process launch, plus an injected live server that rejects upgrade and must be reaped with a failed receipt. The selected probe uses exclusive output creation and preserves its failure reason. Existing manual services and the three pre-existing dirty source files remain untouched.

Base revision 084a962. Raw final receipts/server logs/stdout logs, the selected runner/helpers, extended test source and patch are archived. Run python docs/benchmarks/2026-10-18-overload-reuse/verify.py for an offline audit. Reproduce with python scripts/overload-reuse.py --server PATH_TO_NATIVE --output FRESH.json. This extends short acceptance coverage across overload and admission lifetimes without changing the runtime architecture.
