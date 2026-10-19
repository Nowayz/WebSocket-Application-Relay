import json, subprocess
from pathlib import Path
output=Path('artifacts/socket-status-20261019-comparison.json')
assert not output.exists()
record={'passed':False,'runs':[]}
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
    binary='artifacts/candidates/fairness-short-20261019-current/war-relay' if kind=='control' else 'artifacts/candidates/socket-status-20261019-candidate/war-relay'
    receipt=Path(f'artifacts/socket-status-20261019-{label}.json')
    command=['python','scripts/mixed-capacity.py','--server',binary,'--capacity-client','artifacts/candidates/latency-timeline-client/capacity','--sidecar','artifacts/candidates/private-payload-20261019-client/inspection','--workers','8','--users','100000','--seconds','2','--readers','8192','--scope','observers','--window','4','--inspectors','0','--pairs','0','--churn-workers','8','--churn-cycles-per-second','16','--fanout-windows-per-second','16','--exact-fanout-latency','--output',str(receipt)]
    with receipt.with_suffix('.controller.log').open('x') as log: done=subprocess.run(command,stdout=log,stderr=log)
    x=json.loads(receipt.read_text()); assert done.returncode==0 and x['passed'],(label,x.get('error'))
    row={'label':label,'kind':kind,'receipt':str(receipt),'cpu':x['server_cpu_seconds'],'private_maxima_ms':x['latency']['max_ms'],'fanout_maxima_ms':x['fanout_exact_ms']['max_ms'],'fanout_windows':x['fanout_windows']}
    record['runs'].append(row);output.write_text(json.dumps(record,indent=2)+'\n'); print(json.dumps(row),flush=True)
record['passed']=True;output.write_text(json.dumps(record,indent=2)+'\n')
