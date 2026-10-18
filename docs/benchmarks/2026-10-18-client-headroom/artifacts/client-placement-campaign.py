from pathlib import Path
import subprocess,json
record={'passed':False,'scope':'Private-only ablation, NOT mixed-load result: identical100k users,1KiB,4Hz,8s,8 pinned native workers on0-7; same8-thread default61 client binary, workers individually pinned. Compare client8logical CPUs sharing4physical cores versus8distinct physical cores. No sidecar in either configuration, freeing physical cores without overlap. ABBA fresh processes. Other source, counts, server/socket limits unchanged; no concurrent measured work/builds.','runs':[]}
for label,repeat in [('smt',2),('physical',1),('physical',2),('smt',3)]:
 prefix=f'artifacts/client-placement-{label}-{repeat}'
 command=['python3','artifacts/client-placement-diagnostic-capacity.py','--server','artifacts/candidates/mixed-capacity-final/war-relay','--client','artifacts/candidates/client-event-control/capacity','--users','100000','--seconds','8','--messages-per-second','4','--client-workers','8','--user-latency-output',prefix+'.csv','--workers','8','--pin-workers','--pin-client-workers','--server-cpus','0-7','--client-cpus',('8-11,24-27' if label=='smt' else '8-15'),'--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--output',prefix+'.json']
 print('starting',label,repeat,flush=True)
 with open(prefix+'.log','w') as log:r=subprocess.run(command,stdout=log,stderr=log)
 data=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'label':label,'repeat':repeat,'command':command,'receipt':prefix+'.json','exit_code':r.returncode})
 Path('artifacts/client-placement-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':label,'repeat':repeat,'passed':data['passed'],'measurement':data.get('measurement')}),flush=True)
 assert r.returncode==0 and data['passed']
record['passed']=True
Path('artifacts/client-placement-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
