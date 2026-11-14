# Short reproducible performance work

Measure the complete workload before changing it. Preserve payload bytes,
per-recipient FIFO, permission/acknowledgment boundaries and memory accounting.
A component win is not a complete-relay win. Optimize latency alongside CPU,
throughput, copied bytes and retained memory; reject changes whose tradeoffs
conflict with consistently responsive users. Current results are in
[performance.md](performance.md); the design is in [architecture.md](architecture.md).

## Small checks before timing

```sh
cargo build --locked --release --bin war-relay --example capacity --example inspection --example datagram_load
cargo test --locked --all-targets
```

Freeze executables and source identities before comparisons. `scripts/build-snapshot.py`
rebuilds a copied Cargo source snapshot, refuses stale Cargo artifacts and records
source/executable digests. Use fresh output filenames and serialize builds and
measurements. The scripts refuse evidence collisions. Ordinary timings should
exclude profilers, fault injection and allocator preloads. Every harness starts
the relay with `--dev-cert` and passes its logged certificate pin to the client.

## Capacity

A small portability check needs no fixed CPU masks:

```sh
python3 scripts/capacity.py --server target/release/war-relay \
  --client target/release/examples/capacity --users 256 --seconds 2 \
  --churn-workers 2 --workers 2 --client-workers 2 \
  --server-cpus '' --client-cpus '' --output artifacts/capacity-short.json
```

The 100,000-session, 1 KiB, four-second contract:

```sh
python3 scripts/capacity.py --server target/release/war-relay \
  --client target/release/examples/capacity \
  --users 100000 --room-size 16 --concurrency 64 \
  --seconds 4 --waves 1 --payload-bytes 1024 --messages-per-second 1 \
  --workers 8 --client-workers 8 --pin-workers --pin-client-workers \
  --server-cpus 0-7 --client-cpus 8-15 \
  --output artifacts/capacity-ordinary.json
```

Use a host with memory headroom and separate generator cores. Keep the steady
phase short; population setup, proof delivery to every session, CSV generation
and complete cleanup still take additional wall time. Inspect each user's
samples and the worst events, not just an aggregate percentile. Intended-send
latency also includes client scheduling lag; send-to-receive latency alone can
hide an overloaded generator. `--kernel-socket-resources` adds per-process
SOCK_DIAG snapshots of the relay's UDP socket at the quiescent gates.

## Mixed traffic and inspection

`scripts/inspection.py` measures CBOR command or HTTP inspection during checked
private traffic, room fanout and churn. Its `--fanout-global-broadcast` mode
checks global publications, independent join/echo/close responsiveness, every
reader and exact cleanup. A closed-loop throughput comparison:

```sh
python3 scripts/inspection.py --server target/release/war-relay \
  --client target/release/examples/inspection --rooms 8192 --mode none \
  --inspectors 0 --churn-workers 0 --pairs 64 --workers 4 --client-workers 8 \
  --seconds 4 --server-cpus 0-3 --client-cpus 4-11 \
  --output artifacts/inspection-private.json
```

Replace `--pairs 64` with `--pairs 0 --fanout-readers 63` for room fanout. The
relay is not CPU-bound in this shape, so record relay CPU per delivery as well as
the delivery rate. [datagram-load.md](datagram-load.md) covers paced open-loop
loads that compare reliable streams with datagrams.

## Impaired networks

`scripts/freshness.py` runs one relay and client inside a named network
namespace (`--isolated-namespace`) and impairs only the relay's UDP port with
netem (delay, jitter, loss, reordering). A reliable bulk value shares the session with paced state updates;
the receipt records the bulk transfer time and the age of the newest state.
`scripts/freshness-matrix.py` repeats it across the clean, loss, jitter and
reorder profiles for both stream and datagram updates, in a temporary namespace
it creates with passwordless sudo for `ip` and `tc`; the workloads run as the
invoking user. Use `--mtu 1500`, because loopback's 64 KiB MTU lets a whole
transfer fit in one initial window. `--client-packet-threshold 3` gives the
client QUIC's stock packet-count loss rule, as a browser might use.

Use each script's `--help` for required arguments and host affinity. Reuse the
same client binaries and workload settings before/after. Run a few short serial
control/candidate/candidate/control trials when placement variation matters.
Report ranges, failures and semantic differences. Do not infer production
guarantees from loopback diagnostics.

## Phase-gated capacity allocations

Repeat that receipt with the replacement allocator to capture every phase gate:

```sh
python scripts/capacity-heap.py --receipt artifacts/capacity-ordinary.json \
  --prefix artifacts/capacity-heap/relay --output artifacts/capacity-heap.json
python scripts/analyze-capacity-heap.py --receipt artifacts/capacity-heap.json \
  --phase ready --output artifacts/capacity-allocation-sites.json
```

The heap tool requires gperftools (`/usr/lib/libtcmalloc.so` by default; select
another installation with `--library`). It preloads only the server, pauses the
client at each gate, and waits for the dump's signal handler to finish. The
`--capture-timeout` watchdog controls the diagnostic collector, not relay
protocol deadlines. The collector retries dump requests when no profile appears
and no signal is pending or active, recording attempts and all gate profiles.
[gperftools 2.18.1's signal handler](https://github.com/gperftools/gperftools/blob/gperftools-2.18.1/src/heap-profiler.cc#L361-L371)
uses `TryLock` and can ignore a request during concurrent heap activity. Automatic
interval dumps are disabled; gate and process-exit snapshots remain distinct.
`python scripts/test-capacity-heap-capture.py` checks recovery against the real
profiler after suppressing the first request. Use fresh output paths and prefixes. The analyzer verifies
the exact executable and profile hashes, resolves captured executable addresses,
and checks that all allocation buckets sum to the profile header. Optimized
inline sites can share function names; this is not complete stack attribution.

Kernel socket counters for the relay's UDP socket are collected separately. For
socket-object estimates, read the current kernel's slab sizes without changing
host settings:

```sh
sudo -n python scripts/socket_resources.py --slab-sizes-only > artifacts/socket-slab-sizes.json
python scripts/capacity-heap.py --receipt artifacts/capacity-ordinary.json \
  --prefix artifacts/capacity-heap-with-sockets/relay \
  --output artifacts/capacity-heap-with-sockets.json \
  --slab-sizes artifacts/socket-slab-sizes.json
```

If slab metadata is unavailable, the receipt records that limitation. Buffer
limits are not allocated bytes, SKMEM counters are not an additive total, and
socket-object estimates exclude other kernel storage. Requested live bytes are
already part of process memory; never add them to RSS. The replacement allocator
and capture overhead exclude heap runs from ordinary CPU, latency and RSS
comparisons. Logical zero resource gauges can precede deferred QUIC retirement;
keep the done gate and the separate process-exit profile distinct.

### Server ACK-frequency policy

The standard server builder uses `webtransport::server_transport_config()`.
It requests an ACK after eight ack-eliciting packets, a maximum ACK delay of
5 ms, and immediate acknowledgement of reordering. These are transport feedback
parameters, not limits on application traffic. The batch request targets the
measured ACK-heavy receive path in room/mixed workloads; the delay bound limits
waiting on sparse traffic. Peers that do not support the extension ignore it.

The same factory disables QUIC's packet-count loss rule, so loss is detected
by the time threshold alone (9/8 of the round trip). The packet rule declares
a packet lost once three later ones are acknowledged, which treats ordinary
reordering as congestion; `packet_threshold(3)` restores it. The time rule
depends on the immediate acknowledgement of reordering requested above.

Callers building custom endpoints retain control over their transport config;
`EndpointIngress::bind` does not overwrite it. A caller can start from this
factory and disable requests with `ack_frequency_config(None)`. The standard
CLI and integration fixtures use the same factory.

ACK-frame counts are not CPU shares, and Quinn I/O counters are not kernel
syscall counts. Fewer acknowledgements must not be described as a universal
datagram improvement: high-rate datagram cases have shown more client UDP drops
with less frequent acknowledgement.

### Resolve callers behind generic allocation symbols

An optimized executable may omit frame pointers. A heap captured with a frame
pointer unwinder can stop at a generic allocation helper and cannot identify
its caller. Do not infer a particular container or type from that symbol alone.
Use an explicitly supported unwinder for a separate diagnostic run:

```sh
python scripts/capacity-heap.py --receipt artifacts/capacity-ordinary.json \
  --prefix artifacts/capacity-unwind/relay --output artifacts/capacity-unwind.json \
  --stacktrace-method libunwind
python scripts/analyze-capacity-heap.py --receipt artifacts/capacity-unwind.json \
  --stacks --output artifacts/capacity-unwind-stacks.json
```

The collector requests verbose profiler startup confirmation and records the
requested and observed methods. An explicit request that is not confirmed fails
the collector rather than silently accepting a fallback. Availability depends
on the installed gperftools build. This can cost substantially more allocation
CPU, so exclude its timing/RSS from ordinary comparisons. Watchdogs are
collector controls, not changes to relay protocol deadlines.

`--stacks` groups by all captured frames inside the exact executable image,
using the same receipt/profile hash and ELF load-bias checks. It cannot recover
omitted or inlined frames and excludes shared-library frames. All live bucket
bytes must still sum to the heap header. Omitting `--stacks` retains the existing
first-frame grouping and site rows.
