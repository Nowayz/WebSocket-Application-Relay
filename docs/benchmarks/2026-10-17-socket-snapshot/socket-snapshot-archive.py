from pathlib import Path
import gzip,hashlib,json,shutil
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-socket-snapshot';out.mkdir(exist_ok=True)
for p in sorted((root/'artifacts').glob('socket-snapshot-*')):
 if not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf']:continue
 target=out/p.name
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for folder,prefix in [('socket-snapshot','candidate'),('latency-timeline-client','client')]:
 for p in (root/'artifacts/candidates'/folder).iterdir():
  if p.suffix in ['.json','.rs','.patch']:shutil.copyfile(p,out/(prefix+'-'+p.name))
for name in ['capacity.py','benchmark.py','capacity_pressure.py','plot-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
shutil.copyfile(root/'examples/capacity.rs',out/'source-capacity.rs')
build=json.loads((out/'candidate-build.json').read_text());client=json.loads((out/'client-build.json').read_text())
assert hashlib.sha256((out/'client-capacity.rs').read_bytes()).hexdigest()==client['source_sha256']
assert hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()==build['source_hashes']['scripts/capacity.py']
for name,source in [('candidate-server.rs','src/server.rs'),('candidate-socket.rs','src/server/socket.rs')]:
 assert hashlib.sha256((out/name).read_bytes()).hexdigest()==build['source_hashes'][source]
for rate in ['400k','600k']:
 summary=json.loads((out/f'socket-snapshot-{rate}-summary.json').read_text());assert summary['passed'] and len(summary['runs'])==4
 for row in summary['runs']:
  p=root/row['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==row['trace_sha256']
  run=json.loads(p.read_text());assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
  assert run['runner_sha256']==build['source_hashes']['scripts/capacity.py']
  assert run['client_sha256']==build['client_sha256']==client['binary_sha256']
  # This trace field explicitly hashes the current workspace, not the frozen executable.
  assert run['client_source_sha256']==hashlib.sha256((out/'source-capacity.rs').read_bytes()).hexdigest()
  assert run['server_sha256']==build['baseline_sha256' if row['kind']=='baseline' else 'server_sha256']
selection=json.loads((root/'artifacts/socket-snapshot-selection.json').read_text())
manifest={'base_revision':build['base_revision'],'candidate_binary_sha256':build['server_sha256'],'baseline_binary_sha256':build['baseline_sha256'],'selection':selection,'client_binary_sha256':client['binary_sha256'],'frozen_client_source_sha256':client['source_sha256'],'workspace_client_source_sha256':hashlib.sha256((out/'source-capacity.rs').read_bytes()).hexdigest(),'client_source_attribution':'Raw traces explicitly label client_source_sha256 as a workspace snapshot; frozen client source is associated through client-build.json and client-capacity.rs. Both are preserved separately.','scope':'Eight matched fresh100k-user ordinary cases at400k/600k,120m verified messages. Same exact client and socket/heartbeat/CPU profiles. Latency includes generator/TCP/client;finite plaintext loopback. Tests/corpus/other acceptance separate from performance timing.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
