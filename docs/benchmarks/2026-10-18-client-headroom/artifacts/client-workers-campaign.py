from pathlib import Path
import subprocess,json
record={'passed':False,'scope':'Capacity client workers4 versus8, same8-logical/4-physical allowed CPUs, same default61event interval binary. Sidecar workers8 unchanged. Previous event experiment controls retained; two4worker repeats followed by final8worker control. Eight-second diagnostic phases; complete100k matrices. No source or server change.','runs':[]}
for label,repeat in [('4',1),('4',2),('8',1)]:
 prefix=f'artifacts/client-workers-{label}-{repeat}'
 command=['python3',('artifacts/client-workers-4-runner.py' if label=='4' else 'scripts/mixed-capacity.py'),'--server','artifacts/candidates/mixed-capacity-final/war-relay','--capacity-client','artifacts/candidates/client-event-control/capacity','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','8','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--latency-stages','--output',prefix+'.json']
 print('starting workers',label,repeat,flush=True)
 with open(prefix+'.log','w') as log:r=subprocess.run(command,stdout=log,stderr=log)
 data=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'workers':int(label),'repeat':repeat,'command':command,'receipt':prefix+'.json','exit_code':r.returncode})
 Path('artifacts/client-workers-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'workers':label,'repeat':repeat,'passed':data['passed'],'client_cpu':data['client_after']['capacity']['cpu_seconds']-data['client_before']['capacity']['cpu_seconds'],'latency':data.get('latency')}),flush=True)
 assert r.returncode==0 and data['passed']
record['passed']=True
Path('artifacts/client-workers-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
