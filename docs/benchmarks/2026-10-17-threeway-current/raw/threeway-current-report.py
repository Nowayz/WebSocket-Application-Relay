#!/usr/bin/env python3
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
DEST=ROOT/'docs/benchmarks/2026-10-17-threeway-current'
d=json.loads((ROOT/'artifacts/threeway-current-summary.json').read_text())
m=d['summary']
def v(case,kind,key):return m[case+'/'+kind][key]['median']
names={'cpp':'Original GitHub C++','rust':'Selected Rust','modern':'Accepted modern C++'}
lines=['# Original GitHub C++ versus current Rust and modern C++','',
'2026-10-17. Fresh three-way measurements of the exact selected/accepted binaries. The counter-layout, array-body, compact-handle and observer-index experiments are excluded. No relay implementation or running service changes.','',
'## Matched workload','',
'Six fresh six-second trials per implementation and workload: 54 trials total. Server CPU set 0–3 and client CPU set 4–11; workers can migrate within their common set, matching the original program’s affinity contract. Every process is isolated in private network/PID namespaces. Candidate and workload order rotate, with each candidate occupying each starting position twice. Ordinary timings are sequential and exclude builds, profilers and other load tests.','',
'The same frozen checked client speaks each native protocol without a bridge. Private cases use 64 active pairs plus 8,192 idle single-member rooms (8,321 live connections including the retained inspection socket). Values cycle through 64 distinct 1 KiB application payloads encoded as identical 1,027-byte CBOR byte strings, with 16-message windows. Original C++ forwards these as opaque binary; both current implementations validate CBOR. Envelope widths and control encodings differ naturally between protocols. Query cases add two inspectors at 10 ms intervals, totaling 200 checked complete channel queries/s. Fanout replaces private pairs with one publisher and 63 readers, retaining the 8,192 idle rooms.','',
'CPU is process CPU between ready and measured gates, excluding connection setup, postchecks and retirement. RSS is process resident memory sampled at the measured gate, excluding kernel socket/client memory. Latency includes client scheduling, validation and windowing; p99 values are logarithmic histogram upper bounds, not isolated relay-hop measurements.','',
'## Results','',
'All values are medians of six process trials, retaining the timed data from one Rust query-pacing failure. That trial completed 1,199 of 1,200 scheduled queries and aborted before its postchecks; it is not counted as a fully passed trial. Throughput counts verified delivered messages, including all fanout readers. CPU in query cases includes management work as well as private forwarding. With six repeats, arithmetic medians of histogram upper bounds can fall between buckets; 3.145728 ms is the midpoint of 2.097152 and 4.194304 ms, not a new histogram bucket.','']
for case,title in [('private','Private forwarding'),('query','Private forwarding plus 200 channel queries/s'),('fanout','Fanout to 63 readers')]:
 lines += ['### '+title,'','| Implementation | Deliveries/s | CPU µs/delivery | p99 upper ms | Measured RSS MiB |','|---|---:|---:|---:|---:|']
 for kind in names:
  lines.append(f'| {names[kind]} | {v(case,kind,"per_second"):,.0f} | {v(case,kind,"cpu_us"):.3f} | {v(case,kind,"p99_upper_ms"):.6f} | {v(case,kind,"rss_mib"):.2f} |')
 if case=='query':
  lines+=['', 'Query p99 upper bounds: '+', '.join(f'{names[k]} {v(case,k,"query_p99_upper_ms"):.6f} ms' for k in names)+'.']
 lines+=['']
lines+=['### All-repeat ranges','','| Workload | Implementation | Deliveries/s min–max | CPU µs/delivery min–max |','|---|---|---:|---:|']
for case in ['private','query','fanout']:
 for kind in names:
  row=m[case+'/'+kind];rate=row['per_second'];cpu=row['cpu_us']
  lines.append(f'| {case} | {names[kind]} | {rate["min"]:,.0f}–{rate["max"]:,.0f} | {cpu["min"]:.3f}–{cpu["max"]:.3f} |')
lines+=['','![All process repeats and medians](raw/threeway-current-comparison.png)','',
'## Interpretation and limits','',
'Original C++ has 6.8% more median private throughput than selected Rust and 16.4% more than modern C++; its median CPU cost is 12.8% and 30.1% lower, respectively. Modern C++ has 2.25x the original’s fanout throughput and 2.81x Rust’s, but spends more CPU per delivery. The private-path conclusion must be based on these matched rows, rather than combining the original’s historical rate with a different modern workload. Process ranges overlap; the result describes these implementations on this host and workload, not an intrinsic language ranking. Accepted modern C++ retains a substantial fanout-throughput advantage, which comes with a different CPU-per-delivery tradeoff.','',
f'{d["passed_trials"]} of {d["trials"]} ordinary trials pass every check. All timed windows together contain {d["verified_timed_deliveries"]:,} timed deliveries, {d["verified_timed_queries"]:,} paced queries and {d["normal_closes"]:,} normal Close/EOF exchanges from the fully completed trials. One Rust query trial missed one scheduled query (1,199/1,200), so the initial runner stopped before postchecks; a continuation driver ran only the remaining unstarted cases, without retries or dropped observations. Sender IDs, exact payload bytes, FIFO, complete channel replies and departure notifications are checked. Rust receives its exact counter/byte/reservation checks. Modern C++ receives checks for its exported counter contracts and zero physical resources after retirement. Original C++ has no equivalent management counters; its listeners, thread count, loaded libraries, protocol replies and socket closes are verified.','',
'This campaign does not measure 100,000 users, long-duration stability, TLS, overload or hostile traffic, and does not rerun the separate original-C++ churn failures. Earlier 100,000-user Rust/modern-C++ results remain a distinct experiment. Nothing here establishes original-C++ capacity or reliability at 100,000 users.','',
'## Provenance and replay','',
'- Original upstream commit `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`, rebuilt with four workers. Only the temporary source’s worker-count expression differs from upstream; routing/storage/protocol/uWS sources match recorded hashes. This is not the repository’s old shipped executable with failed listener binds.',
'- Original C++ executable: `0f07d539c145f0be960af3b27649126d3bafd7ec2015e6c9e2444cde5fe587b4`.',
'- Selected Rust executable: `f1bf8bf9fd7c7e8f189806dba4bbdc4441e11ca9f7f680d8d14efe4716cb49be` (runtime `c7bb20a103d3266a47ff3f26a6ec1c473d484e5c`).',
'- Accepted modern C++ executable: `b9dc20b92e7c09f2188d1780da8d691730112f442e60447de62ee6b98ae86da1`.',
'- Frozen client executable: `c69a691b6e2cb12ea8ffd566eb7998ace9123c6c34815ee577be87da79f48569`.',
'- C++ compiler/build flags, pinned modern dependencies, Rust compiler/source hashes, reconstructed selected Rust sources and original/modern source snapshots are included. This compares the recorded release builds, not identical compiler backends or CPU-target settings. All three use glibc; AOCL was not selected by previous studies.',
'- `reproduce.json` records the command and artifact hashes. `raw/threeway-current.json` preserves the matrix; each trace retains commands, gates, resource snapshots, checks and logs. `raw/threeway-current-summary.json` preserves every repeat and aggregate as well as median metrics.',
'- Preliminary smoke failures are retained as harness diagnostics only: live cork-buffer accounting is not a zero-buffer guarantee, C++ value-byte totals include controls, physical retirement can lag the client gate, and fanout without private health rooms has no stored variable. These failed assumptions were corrected before the ordinary campaign; they are not failed relay-performance trials. Historical smoke runner hashes can differ from the final archived runner.',
'- Run `python3 verify.py` to verify the archive hashes and selected source/build contracts. ELF files are identified by digest but not committed.','']
(DEST/'README.md').write_text('\n'.join(lines))
