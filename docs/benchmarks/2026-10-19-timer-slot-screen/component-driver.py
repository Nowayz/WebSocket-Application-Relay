import hashlib,json,statistics,subprocess
from pathlib import Path
out=Path('artifacts/timer-slot-20261019-results.json');assert not out.exists()
record={'passed':False,'runs':[]}
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
 binary=Path(f'artifacts/candidates/timer-slot-20261019-{kind}/measurement')
 command=['taskset','-c','0',str(binary),'timer_slot_measurement','--ignored','--nocapture','--test-threads=1']
 r=subprocess.run(command,capture_output=True,text=True,timeout=10)
 Path(f'artifacts/timer-slot-20261019-{label}.log').write_text(r.stdout+r.stderr)
 assert r.returncode==0,(label,r.stdout,r.stderr)
 rows=[json.loads(s.split('TIMER_SLOT_MEASUREMENT ',1)[1]) for s in r.stdout.splitlines() if 'TIMER_SLOT_MEASUREMENT ' in s]
 assert len(rows)==1
 x=rows[0];assert x['connections']==1024 and x['iterations_per_batch']==100 and len(x['ns_per_scheduled_timer'])==7
 row={'label':label,'kind':kind,'command':command,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'measurement':x,'median_ns_per_timer':statistics.median(x['ns_per_scheduled_timer'])}
 record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
