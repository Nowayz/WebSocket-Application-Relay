import json,subprocess,sys,time
from pathlib import Path
root=Path('/home/phil/dev/war_next');work=Path('/home/phil/.codex/worktrees/departure-quantum/war_next');out=root/'artifacts/departure-pressure-fixture-20261019-final'
checks=[]
for label,audience,extra,expected in [('default',8192,[],0),('pressure',100000,['--queue-budget-bytes','262144'],1)]:
 command=['taskset','-c','8-15,24-31',sys.executable,str(work/'scripts/departure-fairness.py'),'--server',str(root/'artifacts/candidates/departure-fifo-20261019-selected/war-relay'),'--audience',str(audience),'--output',str(out/(label+'.json'))]+extra
 start=time.monotonic()
 with (out/(label+'.log')).open('x') as log:r=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
 x=json.loads((out/(label+'.json')).read_text())
 assert r.returncode==expected and x['server_exit']==0 and not x['server_forced_kill']
 if label=='default':assert x['passed'] and x['queue_budget_bytes'] is None
 else:assert not x['passed'] and int(x['failure_metrics']['war_global_pressure_total'])>0 and '--total-queue-bytes' in x['server_command']
 checks.append({'case':label,'command':command,'exit':r.returncode,'expected_exit':expected,'elapsed_seconds':time.monotonic()-start})
 print(label,'verified',flush=True)
(out/'validation.json').write_text(json.dumps(checks,indent=2)+'\n')
