import json,subprocess,sys
from pathlib import Path
for label,server in [('control','capacity-final'),('selected','c10m-sendmsg')]:
    output=Path(f'artifacts/c10m-repeat-mixed-{label}.json')
    command=[sys.executable,'scripts/capacity.py','--server',f'artifacts/baselines/{server}/war-relay','--client','artifacts/baselines/capacity-final/capacity','--users','100000','--seconds','65','--churn-workers','16','--ping-interval-secs','15','--connection-headroom','10000','--pressure-client','artifacts/baselines/capacity-final/stress','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--output',str(output)]
    print(json.dumps({'event':'started','candidate':label}),flush=True)
    with output.with_suffix('.log').open('w') as log:
        subprocess.run(command,check=True,stdout=log,stderr=subprocess.STDOUT)
    d=json.loads(output.read_text())
    print(json.dumps({'event':'passed','candidate':label,'measurement':d['measurements'][0],'pressure':d['pressure']['result'] if 'result' in d['pressure'] else d['pressure']}),flush=True)
