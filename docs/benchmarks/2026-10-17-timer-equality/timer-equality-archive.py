from pathlib import Path
import gzip,hashlib,json,shutil
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-timer-equality';out.mkdir(exist_ok=True)
for p in sorted((root/'artifacts').glob('timer-equality-*')):
 if not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf','.rs']:continue
 target=out/p.name
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for folder in ['timer-equality','timer-equality-diagnostic']:
 for p in (root/'artifacts/candidates'/folder).iterdir():
  if p.suffix in ['.json','.patch']:shutil.copyfile(p,out/(folder+'-'+p.name))
for name in ['capacity.py','plot-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
build=json.loads((out/'timer-equality-build.json').read_text())
for name,h in build['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
for name in ['timer-equality-400k-summary.json','timer-equality-600k-summary.json']:
 x=json.loads((out/name).read_text());assert x['passed'] and len(x['runs'])==4
 for r in x['runs']:
  p=root/r['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256'];d=json.loads(p.read_text());assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0
  assert d['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
manifest={'base_revision':build['base_revision'],'candidate_binary_sha256':build['binary_sha256'],'selected':False,'scope':'Rejected Some-only identical timer-key guard after eight matched100k-user trials at400k/600k,120m messages. Diagnostic comparison counts equal optional deadlines including None and includes clock/counter overhead; never ordinary timing evidence. Existing heartbeat-phase c9a6c6da runtime remains selected.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
