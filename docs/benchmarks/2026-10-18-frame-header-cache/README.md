# Reject inline cached WebSocket headers

The native candidate is **unselected**. Original frame and socket source remains
in the workspace and selected services. This short screen tests whether
preparing the immutable WebSocket header once per outgoing frame saves enough
fanout work to justify larger per-recipient metadata.

The prior [outbound-path profile](../2026-10-18-socket-header-screen/README.md)
attributed 9.52% of self user-cycle samples to batched output. That includes
more than header formatting, so it does not establish a potential 9.52% gain.
The candidate tests one concrete component of that work.

## Design and correctness

The control frame stores a ten-byte CBOR prefix separately from its shared
`Bytes` payload. Socket writes build a WebSocket header and copy that small
prefix into stack storage for each recipient/write attempt.

The candidate reserves twenty inline bytes: the WebSocket header ends at byte
ten and the CBOR prefix begins there. After constructing the CBOR prefix, it
caches the corresponding WebSocket length header. Writes borrow that combined
prefix and the unchanged payload as two scatter/gather slices. Header encoding
and its stack arrays disappear from the socket path. Small scalar replies that
extend their CBOR prefix explicitly refresh the cached header before return.
No payload assembly, heap allocation, unsafe code or new synchronization is
introduced. Frame bounds, FIFO, byte quantum and short-write offsets remain.

A targeted test checks known wire-length vectors at 125/126 and 65,535/65,536
bytes, including scalar-prefix refresh. An independent WebSocket decoder
verifies complete bytes. Cloning/retirement retains the original body pointer.
All 80 existing clean library tests, that additional wire test and an ephemeral
layout probe pass: **82 tests total**. The layout probe is not proposed as a
production test. Clean library/native Clippy with `-D warnings` passes.

| Measured layout on this host | Control | Candidate |
| --- | ---: | ---: |
| Frame | 48 bytes | 56 bytes |
| Charged envelope | 56 bytes | 64 bytes |
| Queued pending entry | 80 bytes | 88 bytes |

Thus queued-entry metadata grows 10%, even though payloads remain shared. It
adds no storage to idle sockets with no allocated queue. Queue-capacity metrics
use `size_of::<Pending>()` and therefore include the extra bytes correctly.
These are measured compiler/platform layouts, not a stable Rust ABI guarantee.

## Short whole-relay screen

Clean control and candidate release binaries were built sequentially from
`a1054b5` snapshots in the same project/target paths, excluding all three
pre-existing user overlays. The runtime difference is limited to `src/frame.rs`
and `src/server/socket.rs`. Exact archived candidate source includes a later
`cfg(test)` dependency-path correction that does not change its native code.

Four trials use control/candidate/candidate/control order. Each offers two
seconds of saturating traffic: 64 private pairs, 2,048 fanout recipients and four
messages per checked fanout window. Values are the existing 1 KiB byte strings.
There are no idle listing rooms, inspectors or membership churn. Four native
workers use CPUs 0–3; eight client workers use 4–7,20–23, all individually
pinned. Client and server physical cores are disjoint. No profiling, builds or
other benchmarks overlap traffic.

Full setup, exact Close/departure draining and resource checks are outside the
two-second offer. Explicit departure batches of 32 keep each complete test
short. Every submitted body/sender/FIFO matrix, protocol counter and retirement
check passes; these successes do not imply a performance win.

| Trial | Variant | Private million messages/s | Fanout million deliveries/s | CPU-s / million combined deliveries | Private p99 histogram upper ms | Worst fanout window ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | Control | 1.144 | 0.772 | 1.340 | 2.097 | 14.940 |
| 2 | Cached | 1.198 | 0.683 | 1.369 | 2.097 | 14.613 |
| 3 | Cached | 1.229 | 0.633 | 1.382 | 2.097 | 17.075 |
| 4 | Control | 1.187 | 0.712 | 1.352 | 2.097 | 18.775 |

The candidate shifts work toward private traffic while fanout rates are lower
in both observations. CPU per combined delivery is also higher in both candidate
runs. Fanout maxima overlap and private p99 buckets are identical. The screen
does not demonstrate a low-latency or efficiency gain that justifies the larger
metadata and cache-refresh invariant. It is rejected without a larger or longer
follow-up test. Header formatting is not established as the dominant remaining
bottleneck.

Unpaced work amounts and the private/fanout mixture vary; normalized CPU is not
a fixed-work causal estimate. These finite loopback observations do not prove
a universal regression, long-term per-user tails, TLS/WAN behavior or
100,000-user results. No samples are pooled with earlier studies.

## Reproduction and audit

Raw JSON retains exact commands, binary/client/runner hashes, worker affinity,
phase counters and cleanup results. A representative invocation is:

```sh
python scripts/inspection.py \
  --server artifacts/candidates/frame-header-control/war-relay \
  --client target/release/examples/inspection \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers --output artifacts/frame-header-reproduction.json
```

Use fresh output names; never time candidates concurrently. The frozen rejected
binary is under `artifacts/candidates/frame-header-direct`. CPU masks describe
this host. Native queue/body/read-memory configuration is unchanged.

Receipts are losslessly gzip-compressed. Full control/candidate source,
build/test/lint/layout logs, `summary.json`, `builds.json` and checksums preserve
the experiment. `verify.py` checks archive identities and successful retirement
gates offline, not a new network run. No native source change was selected,
no service restarted, and pre-existing user edits remain byte-identical.
