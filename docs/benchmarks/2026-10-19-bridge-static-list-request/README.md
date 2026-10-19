# Share the constant legacy channel-list request

Selected: the bridge translates its channel-list command using Bytes::from_static for the complete constant CBOR packet [0, [2, null]] (82 00 82 02 f6). Previously it called the general request encoder and allocated a five-byte output buffer each time. This replaces one existing match arm; it adds no branch, field, cache, lock, queue, setting or dependency. The immutable bytes are shared safely for their static lifetime. No routed payload or reply behavior changes.

## Measurement contract

Two clean copied source snapshots at base 48aa325 exclude the three pre-existing dirty relay overlays. The same release conversion example invokes the actual public legacy_bridge::to_cbor function, validates the complete output against protocol::request before measurement, warms 10,000 calls and consumes each output through black_box. Ordinary measurements use CPU 4, seven batches and control/candidate/candidate/control fresh-process order for every command. Batches have 100,000 conversions, except 32 KiB writes with 10,000. Input clone and output drop are included. These are converter measurements, not HTTP/relay latency or throughput; no build, profile or test overlapped timing.

| Command | Control process medians, ns/conversion | Static-list process medians, ns/conversion |
| --- | ---: | ---: |
| Channel list | 36.396–37.109 | 25.728–25.746 |
| Listener flags 7 | 31.447–35.299 | 30.972–31.273 |
| Authenticate, 32 bytes | 31.389–31.811 | 31.730–31.822 |
| Read variable, 32-byte key | 31.614–33.208 | 31.631–33.181 |
| Write variable, 32 KiB | 420.807–427.337 | 399.439–423.024 |

Channel-list conversion is about 30% lower elapsed time in this screen. Unchanged command ranges show run variation and no consistent regression; their apparent improvements are not attributed to this change. Full seven-batch samples and all 20 process outcomes are in timing.json. This is a component selection, not a claim to improve native fanout, network response tails or 100k-user capacity.

## Allocation evidence

Separate tcmalloc/SIGUSR2 diagnostic processes run 1,000 list conversions between quiescent heap captures, after fixtures and an initial conversion establish their owners. Before/after profiles are independently summed from every stack row and checked against the headers and capture hashes. Ordinary timings use the normal allocator and run before these diagnostics; caller profiler environment flags are recorded separately.

| 1,000 conversions | Allocations | Cumulative requested bytes | Live object/byte delta |
| --- | ---: | ---: | ---: |
| Control | 1,000 | 5,000 | 0 / 0 |
| Static list | 0 | 0 | 0 / 0 |

The result removes one allocation and five requested bytes per conversion. It does not make the process or WebSocket transport allocation-free, and cumulative requested bytes are not peak memory or RSS savings. Input storage and transport framing remain separate. Profile before-gate backgrounds differ slightly with process argument lengths, so only within-process deltas are compared.

## Compatibility and integration

The added wire test checks the full list request against the public encoder, preserves the existing silent-ignore behavior for list commands carrying extra arguments, and verifies that byte 2 in an ordinary targeted data packet stays data. Existing decoding/permission/reply rules remain unchanged.

All 139 library tests pass in 10.04 seconds, including the existing real timeout cases. Strict library/test Clippy passes in 2.11 seconds and the modified source passes rustfmt. All 65 JavaScript interoperability checks pass in 1.119 seconds with all six checked resource gauges zero. The actual legacy privileged-command case verifies channel-list response layout/populations and uses repeated list queries as FIFO barriers around ID assignment. These are ordinary debug socket checks, separate from frozen release converter timing.

A complete release bridge is freshly built and frozen as artifacts/candidates/bridge-static-list-20261019-selected/war-legacy-bridge. Existing manually running services, convenience binaries and unrelated source edits were preserved; no process promotion occurred.

## Evidence and reproduction

Compressed base, component candidate and final selected sources, patch, common conversion probe, build receipts, complete timing, allocation analysis, raw compressed profiles, heap receipts and test logs are retained here. The measured candidate excludes the later cfg(test) wire assertion; their production source prefixes are identical. The frozen release examples are artifacts/candidates/bridge-static-list-20261019-{control,candidate}/list_request_conversion. Source manifests differ only in legacy_bridge.rs, with an identical probe.

Run `python docs/benchmarks/2026-10-19-bridge-static-list-request/verify.py` for offline evidence checks. timing.py records the exact CPU pinning, command arguments and order; use fresh artifact outputs before replaying. heap-probe.py takes BINARY FRESH_PROFILE_PREFIX FRESH_RECEIPT OP VALUE and imports scripts/inspection.py from the repository root; OP=2 VALUE=0 selects list conversion. It requires the tcmalloc library recorded in the heap receipts. Keep all builds, timing and diagnostics serial and adapt CPU masks to another host.
