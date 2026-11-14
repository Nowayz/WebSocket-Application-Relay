# Verified open-loop transport load

`examples/datagram_load.rs` and `scripts/datagram-load.py` compare WebTransport reliable streams and datagrams with the same body, routing shape, and arrival verification. Sources generate paced traffic independently of replies. Each recipient checks sender, sequence, generation stamp, and the complete payload. Preallocated sequence bitsets identify unique arrivals, duplicates, reordering, and missing updates without retaining a delivery log.

Build and run a mixed workload:

```sh
cargo build --release --bin war-relay --example datagram_load
python scripts/datagram-load.py --mode datagram \
  --pairs 32 --readers 1024 --private-rate 4000 --fanout-rate 500 \
  --seconds 4 --drain-ms 1000 --payload-bytes 1024 \
  --server-cpus 0-3 --client-cpus 8-15 \
  --output artifacts/webtransport/load-example.json
```

Use `--mode stream` with otherwise identical arguments for the reliable comparison. Use `--readers 0` for private-only traffic, or `--pairs 0` for fanout-only traffic. Private rate is per source; fanout rate is source messages per second, multiplied by recipient count for offered deliveries. Use a fresh output path for every run. Freeze server and client binaries with `--server` and `--client` when comparing server revisions. Run measurements sequentially, without concurrent compilation, profiling, or browser work; rotate variant order across repeated runs.

The runner disables application rate limits for the measured load and derives population limits from the requested session count. It preserves production output queue and byte budgets. It records source and binary hashes, process CPU, phase-gate RSS, owned UDP socket queues/drop counters, server metrics, and client verification results.

## Reading a receipt

- Planned updates are the pacing target. Generated updates count actual source production; `not_generated` and generation lag expose client shortfall.
- Verified unique arrivals during the offer interval determine delivered rate. Drain arrivals are reported separately and cannot inflate this rate.
- Missing datagrams were unreceived within the offer-plus-drain observation window. This does not by itself identify network loss, server admission loss, or socket loss. Kernel UDP drop counters need separate interpretation, including GRO/GSO aggregation.
- Reliable traffic must receive every generated update in FIFO order. A rejection or incomplete run is a failed measurement and remains in the report; it is not a zero-throughput result or an omitted sample.
- Latency histograms have bounded logarithmic bins. Report percentile upper bounds as upper bounds. Generation-to-arrival age and scheduled-to-arrival age answer different questions when the source falls behind.
- CPU cores are process CPU seconds divided by observation elapsed time, including the drain interval. They are not offer-only CPU costs. RSS is sampled at gates, excludes kernel memory, and is not a live-allocation measurement or guaranteed peak.
- Successful receipts verify content, resource counters, normal departure ordering, and final zero resource gauges. Datagram missing updates are allowed by the transport contract; a passed receipt does not establish throughput parity or absence of loss.

Cleanup is outside the observation interval. Fanout departure notices are drained in acknowledged stages so cleanup itself does not manufacture slow-consumer failures. Datagrams are never silently upgraded to reliable delivery; a reliable record during a datagram observation fails the run.

Paced points establish whether a specific offered workload was served. They do not establish maximum sustainable capacity. Use offered-load curves, repeated points, endpoint profiles, and separate 100,000-session memory measurements before claiming parity.
