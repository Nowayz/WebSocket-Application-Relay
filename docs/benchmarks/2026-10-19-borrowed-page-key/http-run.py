import json,subprocess
from pathlib import Path
out=Path('artifacts/page-borrowed-key-20261019-http-comparison.json');assert not out.exists()
record={'passed':False,'runs':[]}
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
 receipt=Path(f'artifacts/page-borrowed-key-20261019-http-{label}.json')
 command=['python','scripts/inspection.py','--server',f'artifacts/candidates/page-borrowed-key-20261019-native-{kind}/war-relay','--client','artifacts/candidates/client-read-window-20261019-control/inspection','--client-source-root','artifacts/client-read-window-20261019-control-source','--mode','http','--page-size','1000','--rooms','8192','--seconds','2','--inspectors','2','--inspection-interval-ms','10','--pairs','64','--client-workers','8','--churn-workers','4','--workers','4','--server-cpus','0-3','--client-cpus','4-7,20-23','--pin-workers','--pin-client-workers','--output',str(receipt)]
 with receipt.with_suffix('.controller.log').open('x') as log:done=subprocess.run(command,stdout=log,stderr=log)
 x=json.loads(receipt.read_text());assert done.returncode==0 and x['passed'],(label,x.get('error'))
 m=x['mixed'];row={'label':label,'kind':kind,'receipt':str(receipt),'server_cpu_seconds':m['server_cpu_seconds'],'healthy':m['healthy'],'inspection':m['inspection'],'churn':m['churn']}
 record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
