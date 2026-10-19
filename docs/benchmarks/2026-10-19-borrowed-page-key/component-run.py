import json,os,statistics,subprocess,hashlib
from pathlib import Path
output=Path('artifacts/page-borrowed-key-20261019-comparison.json');assert not output.exists()
assert 7 in os.sched_getaffinity(0)
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_') and k!='PAGE_HEAP_GATES'}
record={'passed':False,'cpu':7,'runs':[]}
for index,kind in enumerate(['control','candidate','candidate','control']):
 binary=Path(f'artifacts/candidates/page-borrowed-key-20261019-{kind}/page_screen')
 command=['taskset','-c','7',str(binary),'relay::listing::allocation_screen::management_page_screen','--exact','--ignored','--nocapture']
 done=subprocess.run(command,env=env,capture_output=True,text=True)
 log=Path(f'artifacts/page-borrowed-key-20261019-{index}-{kind}.log');log.write_text(done.stdout+done.stderr)
 assert done.returncode==0,done.stdout+done.stderr
 cases=[json.loads(line) for line in done.stdout.splitlines() if line.startswith('{')]
 assert [x['case'] for x in cases]==['first','tail','empty']
 row={'kind':kind,'command':command,'log':str(log),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'cases':cases,'median_ns':{x['case']:statistics.median(x['ns_per_operation']) for x in cases}}
 record['runs'].append(row);output.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'kind':kind,'median_ns':row['median_ns']}),flush=True)
record['passed']=True;output.write_text(json.dumps(record,indent=2)+'\n')
