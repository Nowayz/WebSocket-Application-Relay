from pathlib import Path
import gzip,hashlib,json,shutil
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-heartbeat-phase';out.mkdir(exist_ok=True)
for p in sorted((root/'artifacts').glob('heartbeat-phase-*')):
 if not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf']:continue
 target=out/p.name
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for p in (root/'artifacts/candidates/heartbeat-phase').iterdir():
 if p.suffix in ['.json','.patch']:shutil.copyfile(p,out/('candidate-'+p.name))
for name in ['capacity.py','input-corpus.py','reference.py','plot-latency-timeline.py','plot-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
build=json.loads((out/'candidate-build.json').read_text())
for name,h in build['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
for name in ['heartbeat-phase-400k-summary.json','heartbeat-phase-600k-summary.json','heartbeat-phase-idle-summary.json']:
 x=json.loads((out/name).read_text());assert x['passed']
 for r in x['runs']:
  p=root/r['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256'];d=json.loads(p.read_text());assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0
  assert d['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
manifest={'base_revision':build['base_revision'],'candidate_binary_sha256':build['binary_sha256'],'scope':'Initial heartbeat deadline spread over first interval using existing random sessionID. Earlier firstPing,regular cadence/Pong timeout unchanged. Eight matched ordinary cases400k/600k with120m verified application messages. Separate100k idle pair and65s mixed acceptance. Startup-window observedPing counts differ; no universal latency bound or higher operating-rate claim.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
