# Selected single-frame writer: short unread-consumer check

The selected outlined single-frame writer passed the existing bounded fanout-pressure fixture on 2026-10-18. This is a stability check, not a before/after performance comparison or a 100,000-user acceptance run.

The frozen native binary is source selection 266e5b1, SHA-256 da1d1289c0ce0e5264de26ad87b756d5034576ef3cf4126b44c0f2530c1ea006. Pre-existing dirty root overlays are excluded. All executable identities and full commands are in the receipt; client/source association still requires the respective build records, as the runner states.

The fixture runs 128 stable users in two-user rooms for 25 seconds, the minimum supported pressure duration. An additional ten-member fanout room contains a publisher, one healthy reader and eight unread recipients, with ten seconds of 1 KiB traffic and sixteen messages per window. Setup, buffered unread-frame validation and retirement are additional to measured traffic. It uses four server workers with disjoint server/client CPU masks and no overlapping build or profile.

- Healthy fanout reader: 6,246,464 complete messages verified, including full encoded bytes and ordering. All eight departure notifications received; no teardown capacity closes.
- Unread consumers: all eight retired. Four received Close 1013; four observed transport resets. Every complete buffered frame was validated before each outcome. A partial output frame requires transport retirement rather than appending a Close inside that frame. This does not prove a graceful Close for every slow peer.
- Stable population: 3,200 private traffic probes, plus routing proof for every user before and after pressure; observed private maximum 0.465 ms and histogram p99 upper bound 0.262 ms. These finite loopback measurements include client scheduling and are not latency guarantees.
- Final accounting: exactly eight slow consumers and eight aborted queued outputs; four close failures, zero transport-disconnect counter increments. These counters describe server paths, so a client-observed reset need not increment the server transport-disconnect counter.
- Final connections, sessions, queued reservations, variables, read reservations, allocated native output queue capacity and copied-routing bytes are all zero. Server-owned established TCP census is zero. Server, stable client and stress client all exit normally.

The pressure oracle retains exact input, reuse and lifecycle checks. Output totals are explicitly bounded because intentional cancellation discards queued messages and the stress client does not independently count all welcome/departure bytes; this is not an exact successful-output-byte claim. The original oracle is included for inspection.

Reproduce with a fresh output name:

```sh
python3 scripts/capacity.py \
  --server artifacts/candidates/single-write-outline/war-relay \
  --client target/release/examples/capacity \
  --users 128 --room-size 2 --waves 1 --seconds 25 --active-percent 100 \
  --pressure-client target/release/examples/stress \
  --client-workers 8 --workers 4 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --output artifacts/selected-writer-pressure-repeat.json
```

The frozen path is local to the measurement checkout. Build/freeze the intended implementation explicitly elsewhere and use its executable identity; adapt CPU masks to the host. Run python verify.py to validate the portable archive without repeating traffic. No runtime source, manual service or protected user edit changed during this check.
