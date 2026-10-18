#!/usr/bin/env python3
"""Complete the original planned matrix after a failed case, without retries/drops."""
import hashlib,json,os
from pathlib import Path
import subprocess,sys
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
path=ROOT/'artifacts/threeway-current.json';record=json.loads(path.read_text());a=record['args']
assert sha(ROOT/'scripts/compare-cpp.py')==record['runner_sha256']
record['continuation_runner_sha256']=sha(__file__)
record['continuation_scope']='Continue only previously unstarted planned cases after the retained query pacing failure. No retries, observations dropped, workload or timed measurement changes.'
seen={(r['repetition'],r['case'],r['kind']) for r in record['cases']}
kinds=['cpp','rust','modern'];cases=a['cases']
for rep in range(1,a['repetitions']+1):
 offset=(rep-1)%len(cases)
 for case in cases[offset:]+cases[:offset]:
  first=(rep-1)%len(kinds)
  for kind in kinds[first:]+kinds[:first]:
   if (rep,case,kind) in seen:continue
   isolated=ROOT/f'artifacts/reference/bench-{case}-{kind}-{rep}.json'
   assert not isolated.exists() or isolated.name.startswith('bench-')
   isolated.unlink(missing_ok=True)
   command=['bwrap','--unshare-net','--unshare-pid','--die-with-parent','--ro-bind','/','/','--bind',str(ROOT/'artifacts/reference'),str(ROOT/'artifacts/reference'),'--proc','/proc','--dev','/dev','--',sys.executable,str(ROOT/'scripts/compare-cpp.py'),'--isolated','--host-netns',os.readlink('/proc/self/ns/net'),'--kind',kind,'--case',case]
   for name in ('relay','modern','client','client_source_root','workers','rooms','seconds','pairs','churn_workers','fanout_readers','inspectors','inspection_interval_ms','cpp_executable','cpp_loop_count','server_cpus','client_cpus','client_workers'):
    command += ['--'+name.replace('_','-'),str(a[name])]
   command+=['--cpp-system-libraries','--output',str(isolated)]
   print(json.dumps({'start':kind,'case':case,'repeat':rep}),flush=True)
   completed=subprocess.run(command,cwd=ROOT)
   detail=json.loads(isolated.read_text())
   trace=path.with_name(f'{path.stem}-{case}-{kind}-{rep}.json')
   assert not trace.exists(),trace
   trace.write_text(json.dumps(detail,indent=2)+'\n')
   assert detail['client_sha256']==record['client_sha256']
   assert detail['server_sha256']==record[kind+'_sha256']
   assert detail['runner_sha256']==record['runner_sha256']
   assert (completed.returncode==0)==detail['passed']
   row={'kind':kind,'case':case,'repetition':rep,'trace':str(trace.relative_to(ROOT)),'trace_sha256':sha(trace),'passed':detail['passed']}
   if 'mixed' in detail:
    row.update(mixed=detail['mixed'],sampled_server_rss_kib=[e['server']['VmRSS'] for e in detail['events']],server_threads=detail['events'][0]['server']['Threads'],normal_closes=detail['events'][-1]['details']['counts']['normal_closes'])
   if not detail['passed']:row['error']=detail.get('error')
   record['cases'].append(row)
   path.write_text(json.dumps(record,indent=2)+'\n')
   print(json.dumps({'completed':len(record['cases']),'kind':kind,'case':case,'repeat':rep,'passed':detail['passed'],'error':detail.get('error')}),flush=True)
assert len(record['cases'])==54
record['completed']=True
record['passed']=all(r['passed'] for r in record['cases'])
path.write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'completed':True,'passed':record['passed'],'failed':sum(not r['passed'] for r in record['cases'])}),flush=True)
