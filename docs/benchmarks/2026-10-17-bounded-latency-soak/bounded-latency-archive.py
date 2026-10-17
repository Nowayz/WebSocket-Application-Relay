from pathlib import Path
import gzip,hashlib,json,shutil
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-bounded-latency-soak';out.mkdir(exist_ok=True)
for pattern in ['bounded-latency-*','soak-600-*']:
 for p in sorted((root/'artifacts').glob(pattern)):
  if not p.is_file() or p.suffix not in ['.json','.jsonl','.log','.csv','.py','.rs','.png','.pdf']:continue
  target=out/p.name
  if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
  else:shutil.copyfile(p,target)
for p in (root/'artifacts/candidates/bounded-latency-client').iterdir():
 if p.suffix in ['.json','.rs','.patch']:shutil.copyfile(p,out/('client-'+p.name))
for name in ['capacity.py','plot-user-latency-comparison.py','test-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
run=json.loads((out/'soak-600-bounded.json').read_text());assert run['passed']
build=json.loads((out/'client-build.json').read_text())
assert run['client_sha256']==build['client_sha256']
assert run['client_source_sha256']==build['client_source_sha256']==hashlib.sha256((out/'client-capacity.rs').read_bytes()).hexdigest()
assert run['runner_sha256']==build['harness_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
summary=json.loads((out/'soak-600-analysis.json').read_text());assert summary['passed'] and summary['verified_packet_samples']==240000000
manifest={'base_revision':build['base_revision'],'selected_server_sha256':run['server_sha256'],'client_sha256':run['client_sha256'],'scope':'New bounded per-user diagnostic client,selected relay unchanged.Ten-minute100000-user400k/s stability acceptance,240m exact verified application payloads. User percentiles are histogram upper bounds;maxima exact. No exact-window timeline or old/new relay speedup claim. Histogram value memory bound excludes allocator/socket/generator overhead;processRSS excludes kernel memory.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
