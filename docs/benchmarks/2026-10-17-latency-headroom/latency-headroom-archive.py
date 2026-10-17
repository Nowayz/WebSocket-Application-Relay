from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-latency-headroom';out.mkdir(exist_ok=True)
def put(p,name=None):
 target=out/(name or p.name)
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for p in sorted((root/'artifacts').glob('latency-headroom-*')):
 if p.is_file() and p.suffix in ['.json','.log','.csv','.py','.png','.pdf']:put(p)
for name in ['capacity.py','plot-user-latency-comparison.py','test-user-latency-comparison.py']:
 put(root/'scripts'/name,'source-'+name)
put(root/'artifacts/worker-scaling-latency-eight-1.csv','context-400k.csv')
put(root/'artifacts/worker-scaling-latency-eight-1.json','context-400k.json')
x=json.loads((out/'latency-headroom-summary.json').read_text());assert x['passed'] and len(x['runs'])==4
for r in x['runs']:
 p=root/r['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256']
 d=json.loads(p.read_text());assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0
 assert d['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
 assert len(d['thread_affinity'])==8 and all(t['verified_after_measurement'] for t in d['thread_affinity'])
manifest={'base_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'scope':'Eight pinned workers,100000 users,600k/800k/800k/600k messages/s. Separate send and intended-send user p99s.400k context copied from previous worker-scaling run,not a fresh interleaved baseline. Joint generator/loopback/relay measurement,not isolated server capacity. No runtime change.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
