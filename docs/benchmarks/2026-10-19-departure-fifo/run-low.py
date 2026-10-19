import json, subprocess, sys, time
from pathlib import Path
root=Path('/home/phil/dev/war_next')
out=root/'artifacts/departure-quantum-20261019-runs'
servers={'control':root/'artifacts/candidates/metrics-static-20261019-native-candidate/war-relay',
         'candidate':root/'artifacts/candidates/departure-quantum-20261019-final/war-relay'}
runs=[]
for mode in ['mixed']:
    for index,kind in enumerate(['control','candidate','candidate','control']):
        name=f'{mode}-low-{index+1}-{kind}'
        if mode=='departure':
            command=['taskset','-c','8-15,24-31',sys.executable,str(root/'scripts/departure-fairness.py'),
                     '--server',str(servers[kind]),'--audience','100000','--output',str(out/(name+'.json'))]
        else:
            command=[sys.executable,str(root/'scripts/mixed-capacity.py'),'--server',str(servers[kind]),
                     '--sidecar',str(root/'artifacts/candidates/client-read-window-20261019-control/inspection'),
                     '--capacity-client',str(root/'artifacts/candidates/latency-timeline-client/capacity'),
                     '--users','100000','--seconds','2','--readers','8192','--scope','room',
                     '--pairs','32','--churn-cycles-per-second','32','--fanout-windows-per-second','8',
                     '--pin-client-workers','--output',str(out/(name+'.json'))]
        print('Starting',name,flush=True)
        start=time.monotonic()
        with (out/(name+'.log')).open('x') as log:
            result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
        runs.append({'name':name,'command':command,'exit':result.returncode,'elapsed_seconds':time.monotonic()-start})
        (out/'runs-low.json').write_text(json.dumps(runs,indent=2)+'\n')
        print('Finished',name,result.returncode,round(runs[-1]['elapsed_seconds'],2),flush=True)
        if result.returncode:
            raise SystemExit(result.returncode)
