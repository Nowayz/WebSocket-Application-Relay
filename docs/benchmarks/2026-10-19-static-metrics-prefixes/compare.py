import json,hashlib,os,statistics,subprocess
from pathlib import Path
out=Path('artifacts/metrics-static-20261019-comparison.json');assert not out.exists();assert 8 in os.sched_getaffinity(0)
record={'passed':False,'cpu':8,'runs':[]};expected={}
for index,kind in enumerate(['control','candidate','candidate','control']):
 binary=Path(f'artifacts/candidates/metrics-static-20261019-{kind}/metrics_static_cost')
 for case in ['zero','max','budget']:
  output=Path(f'artifacts/metrics-static-20261019-{index}-{kind}-{case}.prom');assert not output.exists()
  command=['taskset','-c','8',str(binary),case,str(output),'20000','7']
  done=subprocess.run(command,capture_output=True,text=True,check=True)
  row=json.loads(done.stdout);data=output.read_bytes()
  if case in expected:assert data==expected[case]
  else:expected[case]=data
  row.update(kind=kind,command=command,output=str(output),output_sha256=hashlib.sha256(data).hexdigest(),binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),median_ns=statistics.median(row['ns_per_render']))
  record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({'kind':kind,'case':case,'median_ns':row['median_ns'],'length':row['length'],'capacity':row['capacity']}),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
