import subprocess,json
from pathlib import Path
record={'passed':False,'scope':'Matched clean build context; only capacity runtime event_interval changes61 to8. Same frozen native relay, fixed physical CPU sets, full100k mixed4Hz/1KiB and8192fanout4frames16Hz,8churn4Hz,2inspectors. Diagnostic stages ON in both. Four fresh8s phases ABBA; no builds/profiles during measured phases. No server optimization claim.','runs':[]}
for label,repeat in [('control',2),('8',1),('8',2),('control',3)]:
 prefix=f'artifacts/client-event-{label}-{repeat}'
 command=['python3','scripts/mixed-capacity.py','--server','artifacts/candidates/mixed-capacity-final/war-relay','--capacity-client',f'artifacts/candidates/client-event-{label}/capacity','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','8','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--latency-stages','--output',prefix+'.json']
 print('starting',label,repeat,flush=True)
 with open(prefix+'.log','w') as log:r=subprocess.run(command,stdout=log,stderr=log)
 data=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'label':label,'repeat':repeat,'command':command,'receipt':prefix+'.json','exit_code':r.returncode})
 Path('artifacts/client-event-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 timeline=json.loads(Path(prefix+'.windows.json').read_text()) if Path(prefix+'.windows.json').exists() else {}
 print(json.dumps({'done':label,'repeat':repeat,'passed':data['passed'],'client_cpu':data.get('client_after',{}).get('capacity',{}).get('cpu_seconds',0)-data.get('client_before',{}).get('capacity',{}).get('cpu_seconds',0),'latency':data.get('latency'),'after_send_p99_ms':[w['client_stages']['after_send']['p99_ms'] for w in timeline.get('windows',[])]}),flush=True)
 assert r.returncode==0 and data['passed']
record['passed']=True
Path('artifacts/client-event-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
