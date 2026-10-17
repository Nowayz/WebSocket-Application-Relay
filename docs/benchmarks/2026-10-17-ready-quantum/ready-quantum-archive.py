from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-ready-quantum';out.mkdir(exist_ok=True)
def put(p,name=None):
 target=out/(name or p.name)
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for pattern in ['ready-quantum-*','timer-dispatch-*']:
 for p in sorted((root/'artifacts').glob(pattern)):
  if p.is_file() and p.suffix in ['.json','.log','.csv','.py','.png','.pdf','.rs']:put(p)
for folder in ['timer-dispatch-diagnostic','ready-quantum']:
 for p in (root/'artifacts/candidates'/folder).iterdir():
  if p.suffix in ['.json','.patch']:put(p,folder+'-'+p.name)
put(root/'scripts/capacity.py','source-capacity.py')
summary=json.loads((root/'artifacts/ready-quantum-latency-summary.json').read_text());assert summary['passed'] and len(summary['runs'])==4
for run in summary['runs']:
 p=root/run['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==run['trace_sha256']
 x=json.loads(p.read_text());assert x['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest();assert x['passed']
manifest={'base_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'scope':'Diagnostic whole-worker lifetime timer/ready stage measurements, followed by uninstrumented same-workload pinned ready-quantum1024/64/64/1024 comparison. No production change implied. Client and selected binary provenance in previous worker-affinity archive.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
