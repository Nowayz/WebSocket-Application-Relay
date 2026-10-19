import json,subprocess,sys,time
from pathlib import Path
root=Path('/home/phil/dev/war_next');out=root/'artifacts/departure-cursor-20261019-private'
servers={'control':root/'artifacts/candidates/departure-fifo-20261019-selected/war-relay',
         'candidate':root/'artifacts/candidates/departure-cursor-20261019-candidate/war-relay'}
runs=[]
for i,kind in enumerate(['control','candidate','candidate','control']):
 name=f'{i+1}-{kind}';command=[sys.executable,str(root/'scripts/inspection.py'),'--server',str(servers[kind]),
 '--client',str(root/'artifacts/candidates/client-read-window-20261019-control/inspection'),
 '--client-source-root',str(root/'artifacts/client-read-window-20261019-control-source'),
 '--mode','none','--rooms','0','--seconds','2','--inspectors','0','--pairs','64','--churn-workers','0',
 '--workers','4','--client-workers','8','--fanout-readers','0','--server-cpus','0-3',
 '--client-cpus','4-7,20-23','--pin-workers','--pin-client-workers','--output',str(out/(name+'.json'))]
 print('Starting',name,flush=True);began=time.monotonic()
 with (out/(name+'.log')).open('x') as log: result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
 runs.append({'name':name,'command':command,'exit':result.returncode,'elapsed_seconds':time.monotonic()-began})
 (out/'runs.json').write_text(json.dumps(runs,indent=2)+'\n')
 print('Finished',name,result.returncode,flush=True)
 if result.returncode:raise SystemExit(1)
