import json,subprocess
from pathlib import Path
record={'passed':False,'runs':[],'scope':'Capacity client receive collection ablation. Relay frozen3db; identical100k users,4Hz,1KiB,8192fanout readers4-frame16Hz,8churnworkers4Hz,2inspectors10ms. Four fresh8-second runs ABBA. Compare client CPU, complete per-user samples, generator delay and retained client peak RSS. No concurrent builds/profiles.'}
for label,repeat in [('control',1),('lean',1),('lean',2),('control',2)]:
 prefix=f'artifacts/client-join-{label}-{repeat}'
 cmd=['python3','artifacts/client-join-runner.py','--server','artifacts/candidates/mixed-capacity-final/war-relay','--capacity-client',f'artifacts/candidates/client-join-{label}/capacity','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','8','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--output',prefix+'.json']
 print(json.dumps({'starting':label,'repeat':repeat}),flush=True)
 with open(prefix+'.log','w') as log:r=subprocess.run(cmd,stdout=log,stderr=log)
 data=json.loads(Path(prefix+'.json').read_text())
 record['runs'].append({'label':label,'repeat':repeat,'command':cmd,'result':prefix+'.json','exit_code':r.returncode})
 Path('artifacts/client-join-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':label,'repeat':repeat,'passed':data['passed'],'capacity_cpu':data['client_after']['capacity']['cpu_seconds']-data['client_before']['capacity']['cpu_seconds'],'client_peak_rss':data['client_after']['capacity']['peak_rss_kib'],'private':data.get('latency')}),flush=True)
 assert r.returncode==0 and data['passed']
record['passed']=True;Path('artifacts/client-join-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
