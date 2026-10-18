from pathlib import Path
import hashlib,json,statistics
root=Path.cwd();rows=json.loads((root/'artifacts/cbor-exact-encoding-summary.json').read_text())
def median_mean(label,length,clones):return statistics.mean(r['median_ns'] for r in rows if r['label']==label and r['payload']==length and r['clones']==clones)
table=[]
for n in [16,1024,65536]:
 a=median_mean('control',n,0);b=median_mean('tight',n,0)
 table.append(f'| {n:,} | {a:.2f} | {b:.2f} | {(1-b/a)*100:+.1f}% |')
report='''# Exact CBOR byte-string storage (2026-10-18)

## Kept change

Code commit `09f90fb4596b06e576ef5d73eb0149e890216830` changes one reservation expression in `protocol::byte_string`: allocate the actual CBOR length header plus payload instead of nine header bytes plus payload. It adds an ownership/wire boundary test. No pool, alternate allocator, dependency, wire format, native scheduling or routing change is introduced. The rejected receive-collection and scratch-buffer variants are not kept.

For a 1 KiB value, the canonical header is three bytes. The old vector has length 1,027 and capacity 1,033. In pinned `bytes` 1.12.1, `Bytes::from(Vec)` can retain unique boxed storage when length equals capacity; spare capacity instead causes an eager `Shared` ownership allocation. Exact sizing enables that unique-storage path, deferring shared metadata until a clone is actually needed. Clones still share the encoded buffer. The helper still copies its borrowed input into the encoded output, as before; no additional payload copy is introduced and native zero-copy forwarding is untouched.

The local allocator probe reports 1,048 usable bytes for a 1,033-byte request versus 1,032 usable bytes for 1,027. This is a property of this recorded glibc build, not a portable allocator guarantee. The `Bytes` ownership distinction is verified against the pinned dependency source and logical capacity/ownership tests; it is not an allocator-count measurement.

## Evidence and short tests

The eight-second capacity-client profile captures 23,848 samples with zero lost samples. Ring processing, stream polling and waker cloning are visible. Resolving the recorded libc addresses against the matching installed build ID shows samples around `_int_free_chunk` lock acquisition/release. These observations motivate reducing ownership/allocation work; they do not establish the cause of every scheduling stall.

A matched constructor benchmark uses one pinned physical core, identical project path/compiler/dependencies, independent byte-string decode checks before timing, a warmup and seven batches per case. Fresh processes run control / exact / exact / control. Each operation includes construction and destruction with opaque inputs/outputs. The following are means of the two process medians, **without cloning**:

| Payload bytes | Before, ns/op | Exact capacity, ns/op | Reduction in time |
|---:|---:|---:|---:|
'''+ '\n'.join(table)+'''

The clear result is roughly 24–27% less time for the 16-byte and 1 KiB unshared constructors. The 64 KiB ranges overlap and do not establish a speedup. Construction followed by eight clones also has overlapping/variable timings and is not claimed as faster: the first actual clone can require the deferred ownership metadata. Raw batches for both modes are retained. This is an encoding/ownership result, **not a relay throughput or per-user latency result**.

Ten uninstrumented eight-second 100,000-user mixed runs test client changes with the native relay frozen at `3db9ebeb…`, the same 1 KiB values, four probes/user/s, 8,192 fanout readers, four frames/window at 16 Hz, eight churn workers at four cycles/s and two CBOR inspectors. They verify 32 million private probes, all submitted fanout matrices, exact counters, normal Close/EOF and resource/descriptor retirement. Every stable user contributes 32 samples. At that count nearest-rank p99 is the user's maximum; these are screens, not 120-sample p99 comparisons.

- Existing future collection: four controls consume **30.67–32.33 client CPU-s / eight-second phase**.
- `try_join_all` collection: **31.26–31.37 CPU-s**, with lower client peak RSS but no joint CPU/latency improvement. Reject.
- One preparation scratch payload reused across the room: **30.86–31.91 CPU-s**; no clear end-to-end improvement. Reject.
- Exact byte-string sizing, with the original collection/preparation: **30.48–31.20 CPU-s**. The range overlaps controls, so no stable whole-generator CPU percentage or latency improvement is claimed.

All ten runs and their latency excursions remain included. The recorded generator stalls and unstable private tails are not declared fixed by this small allocation change. There is no renewed acceptance of the previous buffered-server candidate. The longest measured network phase here is eight seconds; no soak ran.

The performance controls rebuild the unchanged frozen capacity source in the same project used for variants. Its resulting executable hash differs from the older frozen diagnostic client because its build context differs; do not substitute that older executable as the matched control. Every case records the actual executable hashes. A separate instrumented profile uses the original frozen client and is labelled diagnostic, not an ordinary timing.

## Correctness and actual consumer

The new literal-wire test covers lengths 0, 1, 23, 24, 255, 256, 1,024, 65,535 and 65,536. It checks canonical header bytes, full data preservation after changing the original input, same-buffer cloning and ownership recovery, and no spare capacity in the recovered encoded allocation. Clean selected-source validation passes **78 library tests**. The workspace additionally retains three pre-existing index tests and passes **81 library, three capacity and four inspection tests**, strict all-target Clippy and formatting.

The legacy bridge is a real production-code consumer of `protocol::byte_string`. A clean build from the code commit passes six real JavaScript/legacy interoperability tests, including the unchanged original `relay.js`, binary/text interchange, privileged commands, ID takeover and Close handling. The application-idle test is explicitly skipped because this encoding test disables idle expiry; that policy is unchanged. Both temporary processes exit zero and all native resource gauges return to zero.

The local manual bridge is updated from a clean, frozen executable only after checking its identity, no established bridge clients, and zero native connection/session/queued/variable state. Native relay PID 966632 / executable `3db9ebeb…` and the static UI remain unchanged. The new bridge executable is `8e8b5c771bebcf9ef055b3db8ff1c986521303211ecdf6e338df2b1b4223fc72`, source commit `09f90fb…`. Its live probe verifies full 1 KiB private and broadcast bodies, departure notification, two exact normal Close/EOF handshakes and resource retirement. There is no measured bridge-throughput claim.

The first live probe used an invalid 18-byte legacy channel name. The bridge correctly rejected it under its existing 16-byte compatibility limit; automatic rollback restored the prior process. The corrected 13-byte-channel probe passes. Both the failed fixture/rollback receipt and successful adoption receipt are retained. No live user connection was interrupted.

## Reproduce and provenance

`manifest.json` protects every archived file and decoded CSV. `verify.py` verifies hashes and all ten complete population matrices and recomputes per-user quantile summaries; instrumented profiling is counted separately. All original dirty C++/relay/index-test files remain byte-identical, and none is included in the code commit.

Full baseline project/dependency sources are supplied by the immutable preceding `2026-10-18-buffered-fanout` archive. `dependency.json` binds that manifest. Apply the included protocol or capacity example source to its capacity-client project, using the same project directory for before/after builds. Binary snapshots and actual SHA-256 pins are included. Copy the saved runners under repository `artifacts/` before replaying their recorded commands: those scripts deliberately derive the project root from their parent. The CPU sets are host-specific; verify topology before adapting them.

The kept `examples/encode_cost.rs` runs the constructor/clone benchmark on today's library:

```sh
cargo build --release --example encode_cost
taskset -c 8 target/release/examples/encode_cost
```

Do not compare it with old whole-relay throughput numbers. To evaluate another client build under the population workload, pass it explicitly through `scripts/mixed-capacity.py --capacity-client /path/to/pinned/capacity`. The old frozen default remains unchanged for historical comparability.

This small ownership improvement is complete and verified. Further work on private-user latency still needs to distinguish generator scheduling from relay/kernel delay, using targeted diagnostic windows and both send-relative and intended-send measurements.
'''
(root/'artifacts/cbor-exact-report.md').write_text(report)
