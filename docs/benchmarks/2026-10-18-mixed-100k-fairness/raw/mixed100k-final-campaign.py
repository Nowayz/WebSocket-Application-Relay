import hashlib,json,subprocess,time
from pathlib import Path
root=Path.cwd()
servers={'before':('rust','artifacts/candidates/rust-owner-fanout-explicit/war-relay'),'after':('rust','artifacts/candidates/mixed-capacity-final/war-relay'),'cpp':('cpp','artifacts/candidates/cpp-accounting-layout-control/war-relay-cpp')}
record={'passed':False,'scope':'100000 stable users,400000 independently verified private probes/s,8192 room readers,4-frame windows at16Hz,8 churn workers at4cycles/s each,2 authenticated inspectors at10ms. No extra saturated private pair. Two fresh rotated repeats, physical core separation, full FIFO/body/counter/Close/EOF/retirement checks.','runner_sha256':hashlib.sha256(Path('scripts/mixed-capacity.py').read_bytes()).hexdigest(),'runs':[]}
for repeat,order in [(1,['before','after','cpp']),(2,['cpp','after','before'])]:
 for label in order:
  kind,path=servers[label];prefix=f'artifacts/mixed100k-final-{label}-{repeat}'
  command=['python3','scripts/mixed-capacity.py','--kind',kind,'--server',path,'--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--output',prefix+'.json']
  print(json.dumps({'start':label,'repeat':repeat}),flush=True)
  began=time.monotonic()
  with open(prefix+'.log','w') as log:result=subprocess.run(command,stdout=log,stderr=log)
  item={'kind':label,'repeat':repeat,'command':command,'exit_code':result.returncode,'wall_seconds':time.monotonic()-began,'result':prefix+'.json'}
  record['runs'].append(item);Path('artifacts/mixed100k-final-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
  row=json.loads(Path(prefix+'.json').read_text())
  print(json.dumps({**item,'passed':row['passed'],'latency':row.get('latency'),'fanout_windows':row.get('fanout_windows')}),flush=True)
  assert result.returncode==0 and row['passed'],Path(prefix+'.log').read_text()
record['passed']=True;Path('artifacts/mixed100k-final-campaign.json').write_text(json.dumps(record,indent=2)+'\n')
