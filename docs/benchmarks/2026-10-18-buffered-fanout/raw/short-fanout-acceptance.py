import json,subprocess,time
from pathlib import Path
record={'passed':False,'runs':[]}
base=['python3','scripts/mixed-capacity.py','--server','artifacts/candidates/short-fanout-batch-selected/war-relay','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','8','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency']
cases=[('selected-confirm',base),('membership',base+['--inspectors','0','--fanout-churn-every','8']),('quiet',['python3','artifacts/short-fanout-quiet-compare.py','--candidate','artifacts/candidates/short-fanout-batch-selected/war-relay','--seconds','8'])]
for name,cmd in cases:
 prefix='artifacts/short-fanout-'+name
 cmd=cmd+['--output',prefix+'.json']
 print(json.dumps({'starting':name}),flush=True)
 with open(prefix+'.log','w') as log:
  result=subprocess.run(cmd,stdout=log,stderr=log)
 row=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'name':name,'command':cmd,'exit_code':result.returncode,'result':prefix+'.json'})
 Path('artifacts/short-fanout-acceptance.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':name,'passed':row['passed']}),flush=True)
 assert result.returncode==0 and row['passed'],Path(prefix+'.log').read_text()
record['passed']=True
Path('artifacts/short-fanout-acceptance.json').write_text(json.dumps(record,indent=2)+'\n')
