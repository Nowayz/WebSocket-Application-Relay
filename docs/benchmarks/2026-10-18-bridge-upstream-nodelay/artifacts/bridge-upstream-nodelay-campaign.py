from pathlib import Path
import subprocess,json,statistics
runs=[]
for label,repeat in [('control',1),('candidate',1),('candidate',2),('control',2)]:
 output=f'artifacts/bridge-upstream-nodelay-{label}-{repeat}.json'
 command=['python3','artifacts/bridge-nagle-probe.py','--bridge',f'artifacts/candidates/bridge-upstream-nodelay-{label}/war-legacy-bridge','--output',output]
 with open(output.removesuffix('.json')+'.log','w') as log:r=subprocess.run(command,stdout=log,stderr=log)
 x=json.loads(Path(output).read_text());assert r.returncode==0 and x['passed']
 values=[t['second_forward_ms'] for t in x['trials']]
 runs.append({'label':label,'repeat':repeat,'command':command,'receipt':output,'median_second_forward_ms':statistics.median(values),'max_second_forward_ms':max(values)})
 print(label,repeat,'median',statistics.median(values),'max',max(values),flush=True)
Path('artifacts/bridge-upstream-nodelay-campaign.json').write_text(json.dumps({'passed':True,'scope':'Matched source/build context, control/candidate/candidate/control; eight delayed-ACK trials per fresh process, no concurrent profiling/builds','runs':runs},indent=2)+'\n')
