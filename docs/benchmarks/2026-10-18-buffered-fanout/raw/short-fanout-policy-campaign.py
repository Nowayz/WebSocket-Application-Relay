import json,subprocess
from pathlib import Path
record={'passed':False,'runs':[],'contract':'Final policy-safe binary:8-second mixed confirmation,30-second selected/control pair for120 private probes/user,4-second membership check. Exact checked matrices, output retirement, no soak.'}
base=['python3','scripts/mixed-capacity.py','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency']
for name,kind,seconds,extra in [('policy-8','short-fanout-policy-final',8,[]),('policy-30','short-fanout-policy-final',30,[]),('policy-control-30','mixed-capacity-final',30,[]),('policy-membership','short-fanout-policy-final',4,['--inspectors','0','--fanout-churn-every','8'])]:
 prefix='artifacts/short-fanout-'+name
 cmd=base+['--server',f'artifacts/candidates/{kind}/war-relay','--seconds',str(seconds),'--output',prefix+'.json']+extra
 print(json.dumps({'starting':name}),flush=True)
 with open(prefix+'.log','w') as log: result=subprocess.run(cmd,stdout=log,stderr=log)
 row=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'name':name,'command':cmd,'exit_code':result.returncode,'result':prefix+'.json'})
 Path('artifacts/short-fanout-policy-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':name,'passed':row['passed'],'cpu':row.get('server_cpu_seconds'),'private':row.get('latency'),'fanout':row.get('fanout_exact_ms')}),flush=True)
 assert result.returncode==0 and row['passed'],Path(prefix+'.log').read_text()
record['passed']=True
Path('artifacts/short-fanout-policy-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
