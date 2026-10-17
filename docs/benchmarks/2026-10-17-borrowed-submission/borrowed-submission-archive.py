from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-borrowed-submission';out.mkdir(exist_ok=True)
def put(p,name=None):
 target=out/(name or p.name)
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for pattern in ['borrowed-submission-*','client-placement-*']:
 for p in sorted((root/'artifacts').glob(pattern)):
  if p.is_file() and p.suffix in ['.json','.log','.csv','.py','.png','.pdf']:put(p)
for p in (root/'artifacts/candidates/borrowed-submission').iterdir():
 if p.suffix in ['.json','.patch']:put(p,'candidate-'+p.name)
for name in ['capacity.py','input-corpus.py','reference.py','plot-user-latency-comparison.py']:
 put(root/'scripts'/name,'source-'+name)
build=json.loads((out/'candidate-build.json').read_text())
for name,digest in build['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==digest,name
for name in ['borrowed-submission-latency-v2-summary.json','borrowed-submission-600k-summary.json','client-placement-summary.json']:
 x=json.loads((out/name).read_text());assert x['passed'] and len(x['runs'])==4
 for r in x['runs']:
  p=root/r['trace'];assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256'];d=json.loads(p.read_text());assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0
  assert d['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
manifest={'base_revision':build['base_revision'],'candidate_binary_sha256':build['binary_sha256'],'scope':'Borrowed charged submissions: owners cloned only on deferral. Eight ordinary baseline/candidate cases at400k and600k messages/s (120m messages). Separate client-placement ablation72m. Failed initial candidate launch lacked execute permission; v2 fresh runset replaces incomplete original. Initial corpus CLI workers8 is unsupported; final corpus workers4 is separate from eight-worker capacity acceptance. No profiled timings mixed with ordinary comparisons.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
