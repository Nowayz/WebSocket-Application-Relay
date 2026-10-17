from pathlib import Path
import gzip,hashlib,json,shutil
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-observer-quantum';out.mkdir(exist_ok=True)
for name in ['observer-quantum-summary.json','observer-quantum-400k-summary.json','observer-quantum-mixed.json']:
 assert json.loads((root/'artifacts'/name).read_text())['passed'],name
for p in sorted((root/'artifacts').glob('observer-quantum-*')):
 if not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf']:continue
 dest=out/p.name
 if p.suffix=='.csv':dest=dest.with_suffix('.csv.gz');dest.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,dest)
for folder,prefix in [('observer-quantum','server'),('observer-quantum-client','observer-client'),('heartbeat-phase','baseline'),('latency-timeline-client','capacity-client')]:
 for p in (root/'artifacts/candidates'/folder).iterdir():
  if p.is_file() and p.suffix in ['.json','.rs','.patch']:shutil.copyfile(p,out/(prefix+'-'+p.name))
for name in ['inspection.py','capacity.py','benchmark.py','capacity_pressure.py','plot-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
shutil.copyfile(root/'examples/capacity.rs',out/'source-capacity.rs')
for src,name in [('build.json','mixed-clients-build.json'),('runtime.patch','mixed-clients-runtime.patch'),('examples/capacity.rs','mixed-client-capacity.rs'),('examples/stress.rs','mixed-client-stress.rs')]:shutil.copyfile(root/'artifacts/baselines/capacity-final'/src,out/name)
mixed_overlays={'examples/capacity.rs':'mixed-client-capacity.rs','scripts/capacity.py':'mixed-source-capacity.py','scripts/capacity_pressure.py':'mixed-source-capacity_pressure.py'}
for src,name in mixed_overlays.items():shutil.copyfile(root/'artifacts/baselines/capacity-final'/src,out/name)
build=json.loads((out/'server-build.json').read_text());client=json.loads((out/'observer-client-build.json').read_text());cap=json.loads((out/'capacity-client-build.json').read_text());mixed_clients=json.loads((out/'mixed-clients-build.json').read_text())
for name,want in build['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==want,name
for name,want in client['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==want,name
assert hashlib.sha256((out/'observer-client-inspection.rs').read_bytes()).hexdigest()==client['sources']['examples/inspection.rs']
assert hashlib.sha256((out/'observer-client-fanout.rs').read_bytes()).hexdigest()==client['sources']['examples/inspection/fanout.rs']
assert hashlib.sha256((out/'capacity-client-capacity.rs').read_bytes()).hexdigest()==cap['source_sha256']
for name in ['capacity','stress']:assert hashlib.sha256((out/('mixed-client-'+name+'.rs')).read_bytes()).hexdigest()==mixed_clients['files']['examples/'+name+'.rs']
for name in ['observer-quantum-summary.json','observer-quantum-400k-summary.json']:
 summary=json.loads((out/name).read_text())
 for r in summary['runs']:
  p=root/r['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256'];run=json.loads(p.read_text())
  assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
  assert run['server_sha256']==(build['binary_sha256'] if r['kind']=='candidate' else json.loads((out/'baseline-build.json').read_text())['binary_sha256'])
  if 'workers' in r:
   assert run['client_sha256']==client['binary_sha256']
   assert run['client_source_sha256']==client['sources']['examples/inspection.rs']
   assert run['runner_sha256']==hashlib.sha256((out/'source-inspection.py').read_bytes()).hexdigest()
  else:
   assert run['client_sha256']==cap['binary_sha256']
   assert run['client_source_sha256']==hashlib.sha256((out/'source-capacity.rs').read_bytes()).hexdigest()
   assert run['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
mixed=json.loads((out/'observer-quantum-mixed.json').read_text());assert mixed['server_sha256']==build['binary_sha256'] and mixed['client_sha256']==mixed_clients['files']['capacity'] and mixed['pressure_client_sha256']==mixed_clients['files']['stress']
manifest={'base_revision':build['base_revision'],'selected':True,'mixed_build_overlays':mixed_overlays,'selected_server_sha256':build['binary_sha256'],'scope':'Recipient-work accounting for global/observer fanout;complete per-message fanout,FIFO and body ownership preserved. Eight matched512-observer mixed trials;four100k private guards;separate65s100k mixed acceptance. No universal latency guarantee or C++/WAN/TLS speedup. Histograms are bounds;100k CSV percentiles exact. Explicit adverse tails/CPU and fanout-throughput tradeoffs in docs.','client_source_attribution':'Observer client trace source is its frozen snapshot. Capacity raw client_source_sha256 explicitly hashes the current workspace;actual guard/mixed executables bind to original separate build/source records.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
