from pathlib import Path
import subprocess,json
record={'passed':False,'scope':'Fresh clean-source matched builds; only mutex-serialized notification changesRMW toload/store.4096roomfanout readers,1KiB,4framewindows,4s,server4physical0-3/client8physical4-11. Explicit per-thread server/client affinity from validated capacity helper, verified at gates. No profile/build overlaps. Full body/FIFO/counts/retirement. ABBA.','runs':[]}
for label,repeat in [('control',1),('candidate',1),('candidate',2),('control',2)]:
 prefix=f'artifacts/notify-guard-pinned-{label}-{repeat}'
 cmd=['python3','artifacts/notify-guard-inspection-pinned.py','--server',f'artifacts/candidates/notify-guard-{label}/war-relay','--client','artifacts/candidates/short-fanout-client-v2/inspection','--mode','none','--rooms','0','--seconds','4','--inspectors','0','--pairs','0','--churn-workers','0','--workers','4','--client-workers','8','--fanout-readers','4096','--fanout-window','4','--server-cpus','0-3','--client-cpus','4-11','--output',prefix+'.json']
 print('starting',label,repeat,flush=True)
 with open(prefix+'.log','w') as log:r=subprocess.run(cmd,stdout=log,stderr=log)
 x=json.loads(Path(prefix+'.json').read_text());m=x.get('mixed',{});fan=m.get('fanout',{})
 record['runs'].append({'label':label,'repeat':repeat,'command':cmd,'receipt':prefix+'.json','exit_code':r.returncode})
 Path('artifacts/notify-guard-pinned-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({'done':label,'repeat':repeat,'passed':x['passed'],'deliveries_per_second':fan.get('per_second'),'cpu_us_per_delivery':m.get('server_cpu_seconds',0)*1e6/fan.get('verified',1),'fanout':fan}),flush=True)
 assert r.returncode==0 and x['passed']
record['passed']=True;Path('artifacts/notify-guard-pinned-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
