import json,subprocess
from pathlib import Path
record={'passed':False,'checks':[]}
steps=[('membership',['python3','scripts/mixed-capacity.py','--server','artifacts/candidates/short-fanout-ready64-batch/war-relay','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--seconds','4','--inspectors','0','--fanout-churn-every','8','--output','artifacts/short-fanout-selected-membership.json']),('quiet',['python3','artifacts/short-fanout-quiet-compare.py','--candidate','artifacts/candidates/short-fanout-ready64-batch/war-relay','--seconds','8','--repeats','1','--output','artifacts/short-fanout-selected-quiet.json']),('corpus',['python3','scripts/input-corpus.py','--server','artifacts/candidates/short-fanout-ready64-batch/war-relay','--output','artifacts/short-fanout-selected-corpus.json']),('pressure',['python3','artifacts/short-fanout-selected-pressure.py'])]
for name,command in steps:
 print(json.dumps({'starting':name}),flush=True)
 with open(f'artifacts/short-fanout-selected-{name}.log','w') as log:
  result=subprocess.run(command,stdout=log,stderr=log)
 record['checks'].append({'name':name,'command':command,'exit_code':result.returncode})
 Path('artifacts/short-fanout-selected-checks.json').write_text(json.dumps(record,indent=2)+'\n')
 assert result.returncode==0,Path(f'artifacts/short-fanout-selected-{name}.log').read_text()
 print(json.dumps({'done':name}),flush=True)
record['passed']=True
Path('artifacts/short-fanout-selected-checks.json').write_text(json.dumps(record,indent=2)+'\n')
