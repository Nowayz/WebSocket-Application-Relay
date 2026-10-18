import json,subprocess
from pathlib import Path
command=['python3','scripts/mixed-capacity.py','--server','artifacts/candidates/short-fanout-batch2-final/war-relay','--sidecar','artifacts/candidates/short-fanout-client-v2/inspection','--seconds','30','--scope','room','--pairs','0','--churn-cycles-per-second','4','--fanout-windows-per-second','16','--exact-fanout-latency','--output','artifacts/short-fanout-batch2-30.json']
with open('artifacts/short-fanout-batch2-30.log','w') as log:
 result=subprocess.run(command,stdout=log,stderr=log)
row=json.loads(Path('artifacts/short-fanout-batch2-30.json').read_text())
assert result.returncode==0 and row['passed']
print(json.dumps({k:row[k] for k in ['server_cpu_seconds','latency','fanout_exact_ms']}))
