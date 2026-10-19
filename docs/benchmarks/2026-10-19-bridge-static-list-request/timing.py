import json,subprocess
from pathlib import Path
out=Path('artifacts/bridge-static-list-20261019-timing.json');assert not out.exists()
r={'passed':False,'runs':[]}
for op,value in [(2,0),(1,7),(0,32),(5,32),(4,32768)]:
 for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
  binary=Path(f'artifacts/candidates/bridge-static-list-20261019-{kind}/list_request_conversion')
  command=['taskset','-c','4',str(binary),str(op),str(value)]
  done=subprocess.run(command,capture_output=True,text=True,timeout=10);assert done.returncode==0,done.stderr
  result=json.loads(done.stdout)
  row={'label':label,'kind':kind,'command':command,'result':result};r['runs'].append(row)
  out.write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(row),flush=True)
r['passed']=True;out.write_text(json.dumps(r,indent=2)+'\n')
