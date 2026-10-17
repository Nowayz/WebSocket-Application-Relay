from pathlib import Path
import json,subprocess,sys,hashlib
summary={'scope':'Exploratory idle tradeoff:100000 users,0 active application users,30seconds,heartbeat15s,8pinnedserver0-7,8client8-15,16/64KiB buffers. One fresh baseline then phase process,not repeated medians or total-host CPU. Ready/postcheck routes through all users and exact resource retirement retained.','runs':[],'passed':False}
for name in ['baseline','phase']:
 prefix=f'artifacts/heartbeat-phase-idle-{name}'
 assert not Path(prefix+'.json').exists()
 server='artifacts/candidates/'+('borrowed-submission' if name=='baseline' else 'heartbeat-phase')+'/war-relay'
 command=[sys.executable,'scripts/capacity.py','--server',server,'--client','artifacts/baselines/capacity-final/capacity','--users','100000','--seconds','30','--active-percent','0','--workers','8','--client-workers','8','--server-cpus','0-7','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--pin-workers','--output',prefix+'.json']
 print(json.dumps({'kind':name,'command':command}),flush=True)
 with open(prefix+'.log','w') as log:result=subprocess.run(command,stdout=log,stderr=log,timeout=240)
 x=json.loads(Path(prefix+'.json').read_text());assert result.returncode==0 and x['passed'] and x['client_exit_code']==x['server_exit_code']==0
 assert x['measurement']['application_deliveries']==0
 row={'kind':name,'trace':prefix+'.json','trace_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'measurement':x['measurement']}
 summary['runs'].append(row);print(json.dumps(row),flush=True)
 Path('artifacts/heartbeat-phase-idle-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
summary['passed']=True;Path('artifacts/heartbeat-phase-idle-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
