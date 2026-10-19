import hashlib,json,subprocess
from pathlib import Path
out=Path('artifacts/timer-slot-20261019-mixed-results.json');assert not out.exists()
record={'passed':False,'runs':[]}
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
 binary=Path('artifacts/candidates/metrics-static-20261019-native-candidate/war-relay' if kind=='control' else 'artifacts/candidates/timer-slot-20261019-native/war-relay')
 receipt=Path(f'artifacts/timer-slot-20261019-mixed-{label}.json')
 command=['python','scripts/mixed-capacity.py','--server',str(binary),'--capacity-client','artifacts/candidates/latency-timeline-client/capacity','--sidecar','artifacts/candidates/client-read-window-20261019-control/inspection','--users','100000','--seconds','2','--readers','8192','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--output',str(receipt)]
 with receipt.with_suffix('.controller.log').open('x') as log:
  r=subprocess.run(command,stdout=log,stderr=log,timeout=90)
 x=json.loads(receipt.read_text());assert r.returncode==0 and x['passed'],(label,x.get('error'))
 row={'label':label,'kind':kind,'command':command,'receipt':str(receipt),'receipt_sha256':hashlib.sha256(receipt.read_bytes()).hexdigest(),'server_sha256':x['sha256']['server'],'server_cpu_seconds':x['server_cpu_seconds'],'latency':x['latency'],'fanout_exact_ms':x['fanout_exact_ms'],'fanout_windows':x['fanout_windows']}
 record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
