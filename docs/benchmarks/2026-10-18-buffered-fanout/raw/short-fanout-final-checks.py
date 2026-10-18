import json,subprocess
from pathlib import Path
record={'passed':False,'checks':[]}
steps=[('quiet',['python3','artifacts/short-fanout-quiet-compare.py','--candidate','artifacts/candidates/short-fanout-policy-final/war-relay','--seconds','8','--repeats','1','--output','artifacts/short-fanout-policy-quiet.json']),('corpus',['python3','scripts/input-corpus.py','--server','artifacts/candidates/short-fanout-policy-final/war-relay','--output','artifacts/short-fanout-policy-corpus.json']),('pressure',['python3','artifacts/short-fanout-policy-pressure.py'])]
for name,command in steps:
 print(json.dumps({'starting':name}),flush=True)
 with open(f'artifacts/short-fanout-policy-{name}.log','w') as log:
  result=subprocess.run(command,stdout=log,stderr=log)
 record['checks'].append({'name':name,'command':command,'exit_code':result.returncode})
 Path('artifacts/short-fanout-final-checks.json').write_text(json.dumps(record,indent=2)+'\n')
 assert result.returncode==0,Path(f'artifacts/short-fanout-policy-{name}.log').read_text()
 print(json.dumps({'done':name}),flush=True)
record['passed']=True
Path('artifacts/short-fanout-final-checks.json').write_text(json.dumps(record,indent=2)+'\n')
