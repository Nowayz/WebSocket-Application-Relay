# Short stalled-ingress checks

Keep the existing relay behavior and add `scripts/join-pressure.py` as a short real-socket regression tool. The HTTP handshake deadline and prejoin WebSocket deadline both remain absolute: header drips and Ping traffic must not turn them into indefinitely renewable reservations. No runtime, protocol, scheduler or default timeout changes are adopted. Code commit: `ffa9e8f`.

## Observed cases

All servers are fresh disposable instances of the selected native executable, SHA-256 `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`, with four workers. Independent joined clients route verified traffic throughout. These finite plaintext checks test policy/admission/resource behavior, not service latency, production capacity or long-duration stability.

| Pending ingress | Population / waves | Configured deadline | First observed Close/EOF | Last observed Close/EOF |
| --- | --- | ---: | ---: | ---: |
| Silent incomplete HTTP | 1,024 / one wave | 5 s | 5.0001 s | 5.0781 s |
| Dripping incomplete HTTP | 1,024 / one wave; 16 byte rounds at 50 ms spacing | 5 s | 5.0002 s | 5.0717 s |
| Upgraded, no join or Ping | 128 / two waves | 2 s | 2.0000 s | 2.0108 s |
| Upgraded, Ping-active but no join | 128 / two waves | 2 s | 2.0000 s | 2.0110 s |

Times are client observations from fixture-specific opening boundaries, not server-only timestamped deadlines. The tools allow scheduling tolerance and bound the test independently; the table retains actual observed times. The HTTP and WebSocket tools have different traffic/population/measurement contracts and are not compared as latency implementations.

The existing HTTP-pressure helper verifies the full admission cap, one refused excess socket, exact read-reservation bounds, recovery slot reuse, healthy 1 KiB traffic and normal close, and zero final resource gauges. Expected handshake-timeout/admission-rejection counters increase; unrelated error/pressure counters do not. Recovery includes sixteen accepted raw sockets, which are deliberately closed and counted separately from the deadline population.

The new join helper holds a joined 64-byte witness alongside upgraded-but-unjoined peers. Every raw peer must receive the exact 1008 `Channel Join Timeout` Close and acknowledge it through EOF. In Ping mode there is at most one outstanding Ping per peer, each Pong must match its payload, and every peer must have a verified Pong before expiry. Both final Ping waves verify all 128 distinct peers; they check 2,549 and 2,560 Pong frames respectively. New waves and recovery join/self-route/normal-Close probes verify admission reuse. The main silent case verifies 563,969 healthy self echoes; the final full Ping case verifies 564,673. These are checked witness counts, not comparable throughput results.

The final CLI also checks exact incoming bytes, outgoing application bytes counted from independently received frames, welcome/delivery counts, and reused-value bytes. A final 16-peer silent smoke verifies that finished form with 281,138 healthy echoes. The 128-peer silent case precedes only additional byte-accounting/proof/cleanup checks; its precise source snapshot is retained. Python compilation, both modes, nested output-directory creation and final success/failure exit handling are included in the checked implementation. No broad unit or performance suite is substituted for these real wire checks.

All accepted join cases retire connection/session/read-budget/queue/variable/output-capacity gauges to zero, leave only the global channel, and match exact accepted/joined/disconnected/join-rejected/application counters. Only the expected prejoin-rejection counter rises. Unrelated handshake/protocol/close/write/pressure/rejection counters and routing copied bytes remain zero. The healthy witness completes a normal 1000 Close/EOF after the test. No child is forcibly killed.

## Fixture correction retained

The initial fixture attempt failed a combined gate assertion that required zero queued bytes while the healthy witness was still active. That requirement was overstrict: the witness can briefly retain its one 76-byte encoded envelope during a synchronous send. The corrected active gate permits at most that one derived reservation; final retirement still requires exactly zero. The failed receipt, error log and hash-matched original source are retained. This is a fixture correction, not a relay fix or evidence of an application leak. Later runs use selectors rather than `select` to avoid its descriptor-number limit, and final Ping coverage proves distinct peers rather than relying only on an aggregate Pong count.

## Run short checks

```sh
python3 scripts/handshake-pressure.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --client target/release/examples/admission --workers 4 \
  --connections 1024 --kind drip --rounds 16 --period-ms 50 --seconds 6 \
  --output artifacts/handshake-reproduction.json

python3 scripts/join-pressure.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --ping --output artifacts/join-reproduction.json
```

Build the admission example if needed. Use fresh output names; omit `--ping` for silent upgraded peers. The join fixture defaults to 128 peers, two waves and a fixed two-second join deadline, making a default case roughly four seconds plus setup/recovery. `--connections` and `--waves` configure population/repetition, not production policy. Populations must finish setup before the deliberate short deadline; this is a targeted regression fixture, not a large connection-ramp generator. The HTTP helper's CPU masks are host-specific and should be inspected before adaptation.

A WebSocket join deadline starts a policy Close; its TCP admission slot remains occupied until acknowledgement or the close deadline retires the transport. These clients acknowledge promptly. The checks do not claim that an unresponsive closing peer frees its slot immediately at the join deadline. Production defaults remain five seconds for HTTP and ten seconds for joining, with the existing close budget.

Run `python3 docs/benchmarks/2026-10-18-stalled-ingress/verify.py` for offline hash, source, counter, resource and per-peer Pong proof verification. JSON receipts are losslessly gzip compressed. `provenance.json` maps each join receipt to its exact fixture source; other wire helpers are pinned through Git history and their runtime hashes. The selected live native/bridge processes and all three pre-existing user overlays remain unchanged.
