"""Final sequential acceptance; no builds or diagnostics overlap timing trials."""
import json,subprocess,sys
from pathlib import Path
root=Path(__file__).resolve().parents[1]
python=sys.executable
old='artifacts/baselines/capacity-final/war-relay'
new='artifacts/baselines/c10m-sendmsg/war-relay'
client='artifacts/baselines/capacity-final/capacity'
stress='artifacts/baselines/capacity-final/stress'
compare=[python,'scripts/compare.py','--candidate',f'control={old}','--candidate',f'selected={new}','--stress',stress,'--seconds','6','--repetitions','3']
capacity=[python,'scripts/capacity.py','--client',client,'--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536']
tasks=[
 ('dense',compare+['--mode','traffic','--clients','64','--output','artifacts/c10m-final-dense.json']),
 ('fanout',compare+['--mode','fanout','--clients','16','--window','32','--output','artifacts/c10m-final-fanout.json']),
 ('mixed', [python,'scripts/compare.py','--candidate',f'selected={new}','--stress',stress,'--mode','fanout','--clients','16','--window','4','--payload-sizes','32,1024,65520,65536,1048544','--seconds','4','--repetitions','1','--output','artifacts/c10m-final-mixed.json']),
 ('large',capacity+['--server',new,'--users','32','--seconds','2','--payload-bytes','65520','--output','artifacts/c10m-final-large.json']),
 ('400k-control',capacity+['--server',old,'--users','100000','--seconds','10','--messages-per-second','4','--output','artifacts/c10m-final-400k-control.json']),
 ('400k-selected',capacity+['--server',new,'--users','100000','--seconds','10','--messages-per-second','4','--output','artifacts/c10m-final-400k-selected.json']),
 ('100k-mixed',capacity+['--server',new,'--users','100000','--seconds','65','--churn-workers','16','--ping-interval-secs','15','--connection-headroom','10000','--pressure-client',stress,'--output','artifacts/c10m-final-100k-mixed.json']),
]
for name,command in tasks:
    print(json.dumps({'event':'validation_started','case':name}),flush=True)
    with (root/f'artifacts/c10m-final-{name}.log').open('w') as log:
        subprocess.run(command,cwd=root,check=True,stdout=log,stderr=subprocess.STDOUT)
    out=Path(command[command.index('--output')+1])
    data=json.loads((root/out).read_text())
    print(json.dumps({'event':'validation_passed','case':name,'output':str(out),'summary':data.get('summary',data.get('measurements'))}),flush=True)
