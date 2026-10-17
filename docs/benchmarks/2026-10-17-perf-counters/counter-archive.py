from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-perf-counters';out.mkdir(exist_ok=True)
for p in sorted((root/'artifacts').glob('counter-*')):
 if not p.name.startswith(('counter-600k-', 'counter-accounting', 'counter-analysis-', 'counter-smoke', 'counter-harness-', 'counter-cpu-', 'counter-kernel-', 'counter-latency-', 'counter-archive.py')):continue
 if not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf']:continue
 target=out/p.name
 if p.suffix=='.csv' and not p.name.endswith('.perf-stat.csv'):
  target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for name in ['capacity.py','analyze-perf-counters.py','test-perf-counters.py','plot-latency-timeline.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
old=hashlib.sha256((out/'counter-harness-v1.py').read_bytes()).hexdigest();current=hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
for name in ['counter-smoke','counter-600k-1','counter-600k-2','counter-600k-3','counter-accounting']:
 x=json.loads((out/(name+'.json')).read_text());assert x['passed'] and x['client_exit_code']==x['server_exit_code']==0
 assert x['runner_sha256']==(current if name=='counter-accounting' else old)
manifest={'base_revision':'f8faa3680f105e0f423a4b7b35b2a65f8fe61f48','scope':'Counter-only diagnostics,not ordinary comparison timings. Three30s600k repeats plus CPU-accounting follow-up,100000users,8pinned native workers;72m verified messages. Earlier and final exact harness sources retained separately. Proc process CPU,perf task-clock and CPU-set busy time have different scopes; CPU-set totals include other work.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
