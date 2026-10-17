from pathlib import Path
import csv,gzip,hashlib,json,shutil,statistics,subprocess
root=Path.cwd();a=root/'artifacts';out=root/'docs/benchmarks/2026-10-17-cpp-zero-accounting';out.mkdir(parents=True,exist_ok=True)
sha=lambda b:hashlib.sha256(b).hexdigest()
ordinary={name:json.loads((a/('cpp-zero-accounting-'+name+'.json')).read_text()) for name in ['private','one-worker','fanout','100k','100k-confirmation']}
for record in ordinary.values():assert record['passed']
rows=ordinary['100k']['runs']+ordinary['100k-confirmation']['runs'];assert len(rows)==8
for r in rows:
 assert r['passed'] and r['client_exit_code']==r['server_exit_code']==0
 data=(root/r['csv']).read_bytes();assert sha(data)==r['csv_sha256'];values=list(csv.DictReader(data.decode().splitlines()));assert len(values)==100000 and len({v['id'] for v in values})==100000 and all(int(v['samples'])==120 for v in values)
 g={v['event']:v for v in r['gates']};assert g['measured']['details']['counts']['sent_messages']==g['measured']['details']['counts']['deliveries']==12000000
 assert r['after']['fd_count']==r['baseline']['fd_count']
 assert all(r['final_metrics'][key]==0 for key in ['war_connections','war_users','war_queued_bytes','war_variable_bytes','war_buffered_bytes','war_retained_output_bytes'])
for n in ['private','one-worker','fanout']:
 for r in ordinary[n]['runs']:
  assert r['passed'] and r['server_exit_code']==0
  assert all(r['final_metrics'][key]==0 for key in ['war_connections','war_users','war_queued_bytes','war_variable_bytes','war_buffered_bytes','war_retained_output_bytes'])
summary={'decision':'Rejected: no consistent material end-to-end benefit; accepted C++ runtime restored.','timed_population_probes':96000000,'population_cpu':{k:sum(r['server_cpu_seconds'] for r in rows if r['kind']==k) for k in ['control','candidate']},'small_private':{},'passed':True}
for k in ['control','candidate']:
 r=[v for v in ordinary['private']['runs'] if v['kind']==k];n=sum(v['result']['verified_deliveries'] for v in r);summary['small_private'][k]={'cpu_us_per_delivery':sum(v['server_cpu_seconds'] for v in r)/n*1e6,'copy_proxy_range':[min(v['final_metrics']['war_copied_bytes_total']/(1037*v['result']['verified_deliveries']) for v in r),max(v['final_metrics']['war_copied_bytes_total']/(1037*v['result']['verified_deliveries']) for v in r)]}
(a/'cpp-zero-accounting-analysis.json').write_text(json.dumps(summary,indent=2)+'\n')
compressed={}
for p in sorted(a.glob('cpp-zero-accounting-*')):
 if not p.is_file() or '.perf.data' in p.name:continue
 data=p.read_bytes();name=p.name
 if p.suffix=='.csv' or len(data)>256000:
  name+='.gz';data=gzip.compress(data,mtime=0);compressed[name]={'sha256':sha(p.read_bytes()),'bytes':p.stat().st_size}
 (out/name).write_bytes(data)
for p in a.glob('cpp-private-allocation-profile*'):
 if p.is_file():(out/p.name).write_bytes(p.read_bytes())
for kind in ['control','candidate']:
 src=a/'candidates'/('cpp-zero-accounting-'+kind);dest=out/'source'/kind;dest.mkdir(parents=True,exist_ok=True)
 for name in ['build.json','runtime.patch']:
  if (src/name).exists():shutil.copyfile(src/name,dest/name)
 shutil.copytree(src/'cpp',dest/'cpp',dirs_exist_ok=True)
 record=json.loads((dest/'build.json').read_text())
 for name,want in record['source_sha256'].items():assert sha((dest/name).read_bytes())==want,(kind,name)
shutil.copyfile(root/'cpp/compare.py',out/'copy-proxy-runner.py');shutil.copyfile(a/'cpp-copy-proxy-smoke.json',out/'copy-proxy-smoke.json')
lines=['# C++ no-op output-accounting experiment','', '**Decision: reject the runtime prototype; keep the accepted C++ executable and static library.**','', 'The candidate skips atomic updates when buffered/released output bytes are unchanged. It changes no quotas, counter layout, payload ownership, queue shape or protocol rules. It is reconstructed from commit `8119669` plus the saved runtime patch. This intentionally isolates work that had previously been bundled with a rejected counter-layout experiment.','', '## Evidence','', '- Fresh diagnostic profile of the accepted build: `submit` 12.85%, `write` 12.26%, and `reconcile_buffer` 5.25% of sampled cycles. This is diagnostic sampling, not a timing comparison. Perf build IDs and the driver are preserved.','- Eight fresh rotated 8-second private trials with 32 pairs, four individually pinned server workers and the same frozen checked client; 1 KiB payloads, 16-frame windows.','- Four one-worker private guards and four four-worker fanout guards, each six seconds. No consistent CPU-efficiency advantage appears.','- Two independently named four-trial 100,000-user studies. Eight fresh C++ processes, 400,000 checked probes/s for 30 seconds, 120 samples per user: **96,000,000 timed probes verified**. Server CPU is captured between the ready/measured gates; setup, CSV sorting and teardown are excluded.','- All census CSV hashes, unique IDs, 120 samples/user, gate counts, final gauges and exact descriptor retirement are independently checked. The unchanged native corpus also passed 776 cases / 2,311 exchanges on the candidate.','- Ordinary timings are sequential; there is no concurrent build, profile or sanitizer workload. The new proxy-runner test is a separate integration smoke, not a performance result.','', '## 100,000-user results','', '| Study/pair | Control CPU-s | Candidate CPU-s | Candidate change | Control/Candidate p99 of user p99 (ms) | Control/Candidate worst user p99 (ms) |','|---|---:|---:|---:|---|---|']
for study,name in [('initial','100k'),('confirmation','100k-confirmation')]:
 for repeat in [1,2]:
  c=next(r for r in ordinary[name]['runs'] if r['kind']=='control' and r['repeat']==repeat);v=next(r for r in ordinary[name]['runs'] if r['kind']=='candidate' and r['repeat']==repeat)
  lines.append(f"| {study}/{repeat} | {c['server_cpu_seconds']:.2f} | {v['server_cpu_seconds']:.2f} | {(v['server_cpu_seconds']/c['server_cpu_seconds']-1)*100:+.2f}% | {c['p99_ms']['p99']:.3f} / {v['p99_ms']['p99']:.3f} | {c['p99_ms']['max']:.3f} / {v['p99_ms']['max']:.3f} |")
lines += ['', f"Total population CPU is {summary['population_cpu']['control']:.2f} versus {summary['population_cpu']['candidate']:.2f} CPU-seconds: a 0.75% aggregate reduction, with one matched pair reversing the result. This small difference and mixed latency tails do not justify adopting the extra fast-path branches.", '', '## Placement/copy-path confounding','', 'In the small-population private trials, the control used 1.6726 CPU µs per verified delivery and the candidate 1.6714: essentially unchanged. The diagnostic retained-copy proxy ranged from about 58.7% to 87.0% of deliveries across fresh processes. Fresh SO_REUSEPORT socket placement is a material source of workload variation; these runs do not isolate a topology-independent throughput effect.', '', 'The proxy is `war_copied_bytes_total / (1037 * verified_deliveries)` for this 1 KiB private workload. It counts retained copies, including any prefix fallback, local queued work and teardown notifications. It is an approximate copy-path diagnostic, not a census of worker ownership, and must not be compared directly with Rust routing-copy metrics. The final metrics themselves remain the original unmodified measurements.', '', 'Future `cpp/compare.py` traces record the retained-copy bytes per verified operation and this private-workload estimate, with the qualification included in the JSON. A real two-server smoke verified that the field is emitted for C++ without changing the tested runtime.', '', '## Provenance and restoration','', '- Both frozen source trees and build records are under `source/`; runtime binaries are intentionally omitted from Git. The rejected patch and all candidate sources are retained.','- Control executable: `b9dc20b92e7c09f2188d1780da8d691730112f442e60447de62ee6b98ae86da1`.','- Rejected candidate executable: `2c28dcf0a92713febb8d8db235d30cff67901d0dda027c2e3ffabec717294a55`.','- The accepted source, executable, library and build metadata were restored from the frozen control and their source/binary hashes checked. The original C++/Rust comparison archive remains unchanged. No Rust runtime or manual service was replaced.','- Compressed CSV/large-JSON originals have separate uncompressed hashes in `manifest.json`.','', 'Next performance work should control or explicitly characterize connection placement before interpreting small private-path changes, then address the measured cross-worker submission/retention cost. This study does not establish that no-op atomic instructions are free; it establishes that this particular guard change did not earn selection in the measured complete relay.', '', 'Verify this archive with `python3 docs/benchmarks/2026-10-17-cpp-zero-accounting/verify.py`.']
(out/'README.md').write_text('\n'.join(lines)+'\n')
verifier='''from pathlib import Path
import gzip,hashlib,json
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());digest=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert digest((p/name).read_bytes())==want,name
for name,record in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert digest(data)==record['sha256'] and len(data)==record['bytes'],name
for kind in ['control','candidate']:
 b=json.loads((p/'source'/kind/'build.json').read_text())
 for name,want in b['source_sha256'].items():assert digest((p/'source'/kind/name).read_bytes())==want,(kind,name)
print(json.dumps({'passed':True,'files':len(m['files']),'compressed_originals':len(m['compressed_originals']),'source_sets':['control','candidate']}))
'''
(out/'verify.py').write_text(verifier)
manifest={'decision':summary['decision'],'compressed_originals':compressed,'files':{str(p.relative_to(out)):sha(p.read_bytes()) for p in sorted(out.rglob('*')) if p.is_file() and p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print(json.dumps(summary))
