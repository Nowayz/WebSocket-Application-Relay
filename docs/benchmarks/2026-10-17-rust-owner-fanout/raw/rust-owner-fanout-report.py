import json,statistics
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];DEST=ROOT/'docs/benchmarks/2026-10-17-rust-owner-fanout'
d=json.loads((ROOT/'artifacts/rust-owner-fanout-explicit-summary.json').read_text());a=json.loads((ROOT/'artifacts/rust-owner-fanout-adoption.json').read_text())
s=d['final'];names={'rust':'Previous selected Rust','control':'New Rust','modern':'Accepted modern C++','cpp':'Original GitHub C++'}
def m(mode,kind,key):return s[mode+'/'+kind][key]['median']
def bounds(mode,kind,key):
 v=s[mode+'/'+kind][key];return f'{v["min"]:.6f}' if v['min']==v['max'] else f'{v["min"]:.6f}–{v["max"]:.6f}'
lines=['# Rust fanout: output on socket owners, shared bodies, bounded vectored batches','',
f'Accepted runtime commit `{a["source_revision"]}`, executable SHA-256 `{a["native_sha256"]}`. The selected runtime changes two Rust source files. Original C++ and accepted modern C++ are unchanged; prior observer-index and C++ counter-layout experiments remain unselected and preserved.','',
'## Result','',
f'In the final matched fanout campaign, new Rust delivers {m("fanout","control","rate"):,.0f} messages/s: {m("fanout","control","rate")/m("fanout","rust","rate"):.2f}x previous Rust and {m("fanout","control","rate")/m("fanout","modern","rate"):.2f}x modern C++. Every received sender ID, payload and order is checked. Process-to-process placement variation remains visible in the ranges and dots.','',
'| Runtime | Fanout deliveries/s | CPU µs/delivery | Observed p99 histogram upper bounds, ms |','|---|---:|---:|---:|']
for k in ['rust','modern','control','cpp']:lines.append(f'| {names[k]} | {m("fanout",k,"rate"):,.0f} | {m("fanout",k,"cpu_us"):.3f} | {bounds("fanout",k,"p99_upper_ms")} |')
lines+=['','![Fanout throughput and CPU](raw/rust-owner-fanout-throughput.png)','',
'## Cause and implementation','',
'The previous transport attempts every recipient socket write synchronously on the publishing thread. More relay workers do not parallelize that one publisher’s fanout. A fresh eight-second gated cycles:user profile records 53.26% of samples in Socket.send and 16.68% in Peer.enqueue_frame. One I/O worker accumulates 4.18 CPU-seconds and the other three effectively zero. This profile is separate from ordinary timing; sampling is not an off-CPU, kernel-cost or wall-time decomposition.','',
'- Broadcasts and private messages with observers now use an explicit multicast submission method. Socket transports retain charged shared frames and notify their owning workers; the existing single-recipient private method remains immediate. The Frame representation is unchanged.',
'- Worker notifications coalesce while their pending-token queue is nonempty. A sleeping worker still receives a wakeup; every socket retains its existing deduplication and FIFO rules.',
'- Each owner flush uses up to 16 frames / 32 iovecs per syscall, within the existing 256 KiB write quantum. Headers live on the stack; application bodies stay in their original shared receive storage. There is no timer or wait-to-fill threshold.',
'- Short writes retire only fully written frames and retain the original offset, body, byte charge and deadline for the partial frame. Cancellation, queue-message/byte limits, control-frame serialization, sender FIFO and physical-close admission ownership are retained.',
'- Completion statistics aggregate per batch; quota reservations and releases remain immediate and exact. Queue slots allocate on first use, are reused while active, and follow the existing capacity reclamation rules. This is payload sharing, not a claim of zero kernel copying or zero first-use allocation.',
'- The batch writer is explicitly out of line. Generated flush code fell from 2,921 bytes / 1,128 stack-reservation bytes in the first all-fanout variant to 582 / 56 in the selected version. Original flush was 1,460 / 136. Selected and baseline private-send functions both measure 1,508 code bytes / 200 stack-reservation bytes; enqueue_frame both measure 455 / 24. These sizes do not prove instruction identity.','',
'After-profile sampling shows work distributed across the workers and different hot paths: send_fanout, batch writing, wake/queue bookkeeping and worker driving. New code uses safe Rust and adds no dependencies.','',
'## Private forwarding','',
'| Runtime | Private deliveries/s | CPU µs/delivery |','|---|---:|---:|']
for k in ['rust','control','modern','cpp']:lines.append(f'| {names[k]} | {m("private",k,"rate"):,.0f} | {m("private",k,"cpu_us"):.3f} |')
lines+=['','Private throughput is essentially unchanged against previous Rust in the final matched trials. Original-C++ rates vary substantially between fresh processes; these two-repeat private rows do not overturn the earlier six-repeat original-C++ comparison. All lower-rate observations are retained.','',
'## Competing observers, traffic, churn and inspection','',
'Four individually pinned workers, one private pair, 512 global channel observers, 64 idle rooms, eight churn workers, two inspectors querying every 10 ms, six seconds and two fresh trials per implementation. CPU for this case includes setup/cleanup and all competing operations; it is not forwarding-only cost.','',
'| Runtime / trial | Fanout deliveries/s | Private p99 upper ms | Churn p99 upper ms | Inspection p99 upper ms |','|---|---:|---:|---:|---:|']
for r in d['mixed']:
 x=r['result'];n={'rust':'Previous Rust','rust-owner':'New Rust','cpp':'Modern C++'}[r['kind']]
 lines.append(f'| {n} {r["repeat"]} | {x["fanout"]["per_second"]:,.0f} | {x["healthy"]["p99_upper_ms"]:.6f} | {x["churn"]["p99_upper_ms"]:.6f} | {x["inspection"]["p99_upper_ms"]:.6f} |')
lines+=['','## 100,000-user fairness and CPU','',
'Two fresh runs per Rust build, rotated before/after order, eight individually pinned workers, 100,000 live users in 16-member rooms and 400,000 private probes/s for 30 seconds. Every user contributes 120 exact samples. These are 48 million before/after timed probes, including 24 million on the selected new build. Every connection is proved before and after; normal closes, departure notifications, exact sample census, byte/gauge retirement and descriptor counts are checked. Process memory excludes kernel TCP and client memory.','',
'| Runtime / trial | CPU seconds | Populated RSS MiB | Median user p99 ms | p99 of users’ p99 ms | Worst user p99 ms |','|---|---:|---:|---:|---:|---:|']
for r in d['capacity']:
 n='New Rust' if r['kind']=='rust-owner' else 'Previous Rust';p=r['p99_ms'];lines.append(f'| {n} {r["repeat"]} | {r["server_cpu_seconds"]:.2f} | {r["population_rss_kib"]/1024:.2f} | {p["median"]:.6f} | {p["p99"]:.6f} | {p["max"]:.6f} |')
old=sum(r['server_cpu_seconds'] for r in d['capacity'] if r['kind']=='rust');new=sum(r['server_cpu_seconds'] for r in d['capacity'] if r['kind']=='rust-owner')
lines+=['',f'Total private-workload CPU is {new:.2f} versus {old:.2f} CPU-seconds ({(new/old-1)*100:+.2f}%). Median users’ p99 stays around 0.90 ms and tail distributions are comparable across these finite repeats. This establishes no universal private-latency improvement. Actual-send and intended-send per-user values are retained in compressed CSVs.','',
'![Every user and the slowest one percent](raw/rust-owner-fanout-user-fairness.png)','',
'## Validation, iterations and replay','',
'- Selected runtime: 72 unit tests, strict all-target Clippy, formatting and the independent 776-case / 2,311-exchange whole/fragmented/pipelined wire corpus pass. Integrated workspace tests also pass (75 tests, including preserved unselected observer-index tests).',
'- A short-write test alternates WouldBlock and partial writes at several widths across frame boundaries, checking exact FIFO, original body pointers and outstanding byte reservations. Another test proves multicast waits for owner flushing while an ordinary private submission still writes immediately.',
'- The selected ten-second fanout pressure test evicts all eight unread peers (1013 Close or expected transport termination), verifies all healthy frames and all eight departure notices, and retires every queue/read/connection/variable gauge. Healthy traffic continues after the slow peers leave.',
'- The first broadcast-only prototype has four repeats per workload/implementation, 32 fully passed cases. It establishes the large architecture gain. The all-fanout flag version and its outlined-writer follow-up each retain 16 checked saturation cases, six mixed cases and four population cases. Their private-tail concerns led to the selected explicit-method version; neither earlier all-fanout variant was adopted.',
'- The selected explicit version has 16 fully passed saturation cases, six mixed cases, four population cases, the pressure case and a separate fully completed after-profile. Headline saturation values use its two repeats per case. No observations or lower-rate trials are dropped.',
'- Preliminary diagnostics include one profiler-controller SIGINT-status mistake and one rejected pressure-server configuration (queue bytes did not fit the default maximum message). Corrected named runs pass; these setup failures are not benchmark observations.',
'- All ordinary timings are sequential with no builds/profilers/other load tests overlapping. Saturation uses four server CPUs 0–3 and eight client CPUs 4–11; workers may migrate within their shared set to match the original program. Mixed and population cases verify individual worker pinning.',
'- Saturation is 63 readers, 1 KiB values, 16-message windows and 8,192 idle rooms. Original C++ speaks its native binary envelope, both current implementations speak native validated CBOR. One frozen checked client drives all; no bridge. CPU is gated ready-to-measured. Latency includes client scheduling/windowing; histogram limits are upper bounds.',
'- This is plaintext loopback evidence on this host. It does not establish 100,000-user broadcast throughput, a long-duration soak, real-NIC behavior or TLS performance. The 100,000-user cases exercise private probes; fanout is separately checked at 63 and 512 readers.',
'- Runtime commit, exact executable/source/build hashes, all previous iterations, raw traces, profile reports and compressed full-population CSVs are retained. Baseline source resolves through dependency.json to the prior verified three-way archive. Run python3 verify.py for archive/source/CSV integrity; ELF and raw perf.data files remain local.',
'- Native manual service was quiescent before replacement. Its executable hash, readiness, private/fanout delivery, departure notification, normal Close/EOF and zero final resource gauges pass after promotion. The bridge and static UI services keep their processes.','']
(DEST/'README.md').write_text('\n'.join(lines))
