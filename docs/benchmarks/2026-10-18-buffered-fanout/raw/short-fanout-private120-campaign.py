import json,subprocess,time
from pathlib import Path
record={'passed':False,'runs':[],'contract':'30-second measured phases; 100000 private users at4Hz plus8192 room recipients,4 frames16Hz,8 churn workers4Hz,2 CBOR inspectors10ms. Fresh sequential processes; physical server/client core separation. Exact optional fanout per-recipient nearest-rank p99; private120-probe per-user p99 and maxima. No soak.'}
for name,server in [('private120-control','mixed-capacity-final'),('private120-batch','short-fanout-batch-final')]:
 prefix='artifacts/short-fanout-'+name
 cmd=['python3','scripts/mixed-capacity.py','--server',f'artifacts/candidates/{server}/war-relay','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','30','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--output',prefix+'.json']
 print(json.dumps({'starting':name}),flush=True)
 began=time.monotonic()
 with open(prefix+'.log','w') as log:
  result=subprocess.run(cmd,stdout=log,stderr=log)
 row=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'name':name,'command':cmd,'exit_code':result.returncode,'wall_seconds':time.monotonic()-began,'result':prefix+'.json'})
 Path('artifacts/short-fanout-private120-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':name,'passed':row['passed'],'cpu':row.get('server_cpu_seconds'),'private':row.get('latency'),'fanout':row.get('fanout_exact_ms')}),flush=True)
 assert result.returncode==0 and row['passed'],Path(prefix+'.log').read_text()
record['passed']=True
Path('artifacts/short-fanout-private120-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
