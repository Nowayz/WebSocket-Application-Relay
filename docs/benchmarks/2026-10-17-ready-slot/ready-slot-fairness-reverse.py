import csv,json,subprocess,sys,hashlib
from pathlib import Path
root=Path.cwd()
for label,server in [('slot-reverse','artifacts/candidates/ready-slot/war-relay'),('control-reverse','artifacts/candidates/input-batch/war-relay')]:
    seconds,rate=30,4
    command=[sys.executable,'scripts/capacity.py','--server',server,'--client','artifacts/candidates/user-latency-client/capacity','--users','100000','--seconds',str(seconds),'--messages-per-second',str(rate),'--read-storage','worker','--server-cpus','0-3','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--user-latency-output',f'artifacts/ready-slot-fairness-{label}.csv','--output',f'artifacts/ready-slot-fairness-{label}.json']
    print(json.dumps({'started':label,'command':command}),flush=True)
    with open(f'artifacts/ready-slot-fairness-{label}.log','w') as log:
        done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
    data=json.load(open(f'artifacts/ready-slot-fairness-{label}.json'))
    assert done.returncode==0 and data['passed']
    rows=list(csv.DictReader(open(f'artifacts/ready-slot-fairness-{label}.csv')))
    assert len(rows)==100000 and len({r['id'] for r in rows})==100000
    assert len({(r['room'],r['member']) for r in rows})==100000
    assert all(int(r['samples'])==seconds*rate for r in rows)
    assert all(float(r['p50_ms'])<=float(r['p95_ms'])<=float(r['p99_ms'])<=float(r['max_ms'])<=float(r['schedule_max_ms']) for r in rows)
    print(json.dumps({'passed':label,'users':len(rows),'samples_each':seconds*rate}),flush=True)
