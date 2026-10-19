import json,subprocess,sys,time
from pathlib import Path
root=Path('/home/phil/dev/war_next');work=Path('/home/phil/.codex/worktrees/departure-quantum/war_next');out=root/'artifacts/retirement-wake-20261019-runs'
servers={'control':root/'artifacts/candidates/departure-fifo-20261019-selected/war-relay',
         'candidate':root/'artifacts/candidates/retirement-wake-20261019-final-candidate/war-relay'}
runs=[]
cases=[(f'mixed-{i+1}-{kind}',kind,'room') for i,kind in enumerate(['control','candidate','candidate','control'])]
cases += [('mixed-observers-candidate','candidate','observers'),('departure-candidate','candidate',None)]
for name,kind,scope in cases:
 output=out/(name+'.json')
 if scope is None:
  cmd=['taskset','-c','8-15,24-31',sys.executable,str(work/'scripts/departure-fairness.py'),'--server',str(servers[kind]),'--audience','100000','--output',str(output)]
 else:
  cmd=[sys.executable,str(root/'scripts/mixed-capacity.py'),'--server',str(servers[kind]),
       '--sidecar',str(root/'artifacts/candidates/client-read-window-20261019-control/inspection'),
       '--capacity-client',str(root/'artifacts/candidates/latency-timeline-client/capacity'),
       '--users','100000','--seconds','2','--readers','8192','--scope',scope,
       '--pairs','32','--churn-cycles-per-second','32','--fanout-windows-per-second','8',
       '--pin-client-workers','--output',str(output)]
 print('Starting',name,flush=True);began=time.monotonic()
 with (out/(name+'.log')).open('x') as log:r=subprocess.run(cmd,stdout=log,stderr=subprocess.STDOUT)
 runs.append({'name':name,'command':cmd,'exit':r.returncode,'elapsed_seconds':time.monotonic()-began})
 (out/'native-runs.json').write_text(json.dumps(runs,indent=2)+'\n')
 print('Finished',name,r.returncode,round(runs[-1]['elapsed_seconds'],2),flush=True)
 if r.returncode:raise SystemExit(1)
