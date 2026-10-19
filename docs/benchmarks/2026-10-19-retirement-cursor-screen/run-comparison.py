import json,subprocess,sys,time
from pathlib import Path
root=Path('/home/phil/dev/war_next');out=root/'artifacts/departure-cursor-20261019-runs'
servers={'control':root/'artifacts/candidates/departure-fifo-20261019-selected/war-relay',
         'candidate':root/'artifacts/candidates/departure-cursor-20261019-candidate/war-relay'}
runs=[]
for mode in ['pressure','mixed','departure']:
    order=['candidate','control'] if mode=='pressure' else ['control','candidate','candidate','control']
    for i,kind in enumerate(order):
        name=f'{mode}-{i+1}-{kind}';output=out/(name+'.json')
        if mode=='pressure':
            command=['taskset','-c','8-15,24-31',sys.executable,str(root/'artifacts/departure-quantum-20261019-runs/small-budget-check.py'),
                     '--server',str(servers[kind]),'--output',str(output)]
        elif mode=='departure':
            command=['taskset','-c','8-15,24-31',sys.executable,str(root/'scripts/departure-fairness.py'),
                     '--server',str(servers[kind]),'--audience','100000','--output',str(output)]
        else:
            command=[sys.executable,str(root/'scripts/mixed-capacity.py'),'--server',str(servers[kind]),
                     '--sidecar',str(root/'artifacts/candidates/client-read-window-20261019-control/inspection'),
                     '--capacity-client',str(root/'artifacts/candidates/latency-timeline-client/capacity'),
                     '--users','100000','--seconds','2','--readers','8192','--scope','room',
                     '--pairs','32','--churn-cycles-per-second','32','--fanout-windows-per-second','8',
                     '--pin-client-workers','--output',str(output)]
        print('Starting',name,flush=True);started=time.monotonic()
        with (out/(name+'.log')).open('x') as log:
            completed=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
        expected=1 if mode=='pressure' and kind=='control' else 0
        runs.append({'name':name,'command':command,'exit':completed.returncode,'expected_exit':expected,
                     'elapsed_seconds':time.monotonic()-started})
        (out/'runs.json').write_text(json.dumps(runs,indent=2)+'\n')
        print('Finished',name,completed.returncode,round(runs[-1]['elapsed_seconds'],2),flush=True)
        if completed.returncode!=expected: raise SystemExit(1)
