import json,subprocess
from pathlib import Path
out=Path('artifacts/cpu-split-20261019-runs.json');assert not out.exists()
record={'passed':False,'runs':[]}
for mode,pairs,readers in [('private',64,0),('fanout',0,2048)]:
 receipt=Path(f'artifacts/cpu-split-20261019-{mode}.json')
 command=['python','scripts/inspection.py','--server','artifacts/candidates/metrics-static-20261019-native-candidate/war-relay','--client','artifacts/candidates/client-read-window-20261019-control/inspection','--client-source-root','artifacts/client-read-window-20261019-control-source','--mode','none','--rooms','0','--seconds','2','--inspectors','0','--pairs',str(pairs),'--churn-workers','0','--workers','4','--client-workers','8','--fanout-readers',str(readers),'--fanout-window','4','--fanout-close-window','32','--server-cpus','0-3','--client-cpus','4-7,20-23','--pin-workers','--pin-client-workers','--output',str(receipt)]
 with receipt.with_suffix('.controller.log').open('x') as log:r=subprocess.run(command,stdout=log,stderr=log,timeout=30)
 x=json.loads(receipt.read_text());assert r.returncode==0 and x['passed']
 ready=next(e for e in x['events'] if e['event']=='ready')['process'];measured=next(e for e in x['events'] if e['event']=='measured')['process']
 times={key:measured[key]-ready[key] for key in ['cpu_seconds','user_cpu_seconds','system_cpu_seconds']}
 row={'mode':mode,'command':command,'receipt':str(receipt),'cpu':times,'system_share_percent':100*times['system_cpu_seconds']/times['cpu_seconds']}
 record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
