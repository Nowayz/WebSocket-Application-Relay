# Unacknowledged Close: short admission/deadline regression

Extend `scripts/join-pressure.py` with optional `--withhold-close-ack`. Code commit `8b3116f`; no relay runtime or production default changes. This fills the [previous stalled-ingress check's](../2026-10-18-stalled-ingress/README.md) explicit limit: those peers acknowledged their policy Close promptly.

The new mode sets only the disposable fixture server's write/close timeout to one second. At the two-second join deadline, each unjoined peer must receive the exact 1008 `Channel Join Timeout` frame. It deliberately withholds a reply, stops sending further Pings to that closing peer, and waits for EOF. A sampled closing gate must still contain the entire pending population plus the healthy witness, and an excess raw TCP connection must be refused through EOF while that cap is full. After deadline retirement, a new WebSocket must join, self-route and close normally. The ordinary acknowledged mode retains its prior configuration and behavior.

## Observed cases

| Case | Prejoin peers / waves | Missing Close acknowledgements | Verified capacity refusals while closing | Observed Close-to-EOF range |
| --- | --- | ---: | ---: | ---: |
| Silent, no acknowledgement | 128 / one | 128 | 1 | 0.999973–1.001560 s |
| Ping-active before Close, no acknowledgement | 16 / two | 32 | 2 | 0.999976–1.001864 s |
| Ping-active acknowledged control | 16 / one | 0 | 0 | Acknowledgement and EOF verified; no timeout expected |

All cases use the same frozen native executable SHA-256 `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9`, four workers, ephemeral loopback ports, and independently verified 64-byte witness traffic. Input rates, heartbeats and application idle are disabled in the fixture, not changed in production. Each Ping-active wave verifies a Pong from every distinct peer before policy Close. Only one Ping per peer is outstanding. These are finite policy/admission tests, not latency benchmarks or large-population capacity claims; timers use client observation timestamps with scheduling tolerance.

The three cases verify **1,547,621 healthy witness echoes**, four additional recovery echoes, full sender/body/FIFO, application message/byte and reused-value counts, and normal Close/EOF for all joined sessions. At the closing gate, no-ack cases still own 129 or 17 admission slots. After their deadline they return to the one healthy witness; recovery joins then succeed. At final retirement, connection/session/read-buffer/queue/variable/output-capacity gauges are zero and only the global channel remains. Routing copied bytes and unrelated handshake/protocol/write/transport/pressure counters remain zero. Servers exit successfully without forced kills.

Expected telemetry matters: no-ack cases produce exactly **160 `war_close_failures_total`** in aggregate, one per deliberate missing acknowledgement. They also produce exactly three `war_rejected_total` from the explicitly refused excess TCP attempts. The acknowledged control produces neither. These counters are checked rather than silently excluded. A close failure here does not mean a healthy packet delivery or socket write failed.

This establishes observed retirement and admission reuse with these controlled peers. It does not establish a hard latency deadline, behavior for clients continuing arbitrary traffic after Close, TLS/WAN results, or long-duration stability. Kernel TCP state is outside the application resource gauges. Existing production defaults remain a ten-second join timeout and five-second write/close budget.

## Run

```sh
python3 scripts/join-pressure.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --withhold-close-ack --waves 1 \
  --output artifacts/unacknowledged-close-reproduction.json
```

Add `--ping` to exercise checked Ping traffic before Close. Omit `--withhold-close-ack` for the acknowledged control. Fresh output names are required. A default no-ack wave spends about two seconds waiting for join timeout and one second waiting for close retirement, plus setup/recovery. This is a targeted regression fixture; its setup must finish inside the deliberately short join deadline.

`python3 docs/benchmarks/2026-10-18-unacknowledged-close/verify.py` verifies hashes, exact source/byte-counter provenance, sampled held admission, expected refusals/timeout counters, per-peer Pong coverage and final resource gates. JSON receipts are losslessly compressed. The unchanged acknowledged control, Python compilation and success exit statuses pass alongside both no-ack cases. The three original user overlays and selected live native/bridge executable identities remain unchanged.
