import json,subprocess
from pathlib import Path
out=Path('artifacts/client-read-window-20261019-comparison.json');assert not out.exists()
record={'passed':False,'runs':[]}
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
 receipt=Path(f'artifacts/client-read-window-20261019-{label}.json')
 command=['python','scripts/inspection.py','--server','artifacts/candidates/fairness-short-20261019-current/war-relay','--client',f'artifacts/candidates/client-read-window-20261019-{kind}/inspection','--client-source-root',f'artifacts/client-read-window-20261019-{kind}-source','--mode','none','--rooms','0','--seconds','2','--inspectors','0','--pairs','0','--churn-workers','0','--workers','4','--client-workers','8','--fanout-readers','2048','--fanout-window','4','--fanout-close-window','32','--server-cpus','0-3','--client-cpus','4-7,20-23','--pin-workers','--pin-client-workers','--output',str(receipt)]
 with receipt.with_suffix('.controller.log').open('x') as log:done=subprocess.run(command,stdout=log,stderr=log)
 x=json.loads(receipt.read_text());assert done.returncode==0 and x['passed'],(label,x.get('error'))
 m=x['mixed'];n=m['fanout']['verified'];row={'label':label,'kind':kind,'receipt':str(receipt),'deliveries':n,'client_cpu_per_million':m['client_cpu_seconds']*1e6/n,'server_cpu_per_million':m['server_cpu_seconds']*1e6/n,'fanout':m['fanout']}
 record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
