import csv,json,subprocess,sys,hashlib
from pathlib import Path
root=Path.cwd()
for label,seconds,rate in [('normal',60,2),('overload',10,10)]:
    command=[sys.executable,'scripts/capacity.py','--server','artifacts/candidates/input-batch/war-relay','--client','artifacts/candidates/user-latency-client/capacity','--users','100000','--seconds',str(seconds),'--messages-per-second',str(rate),'--read-storage','worker','--server-cpus','0-3','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--user-latency-output',f'artifacts/user-latency-{label}.csv','--output',f'artifacts/user-latency-{label}.json']
    print(json.dumps({'started':label,'command':command}),flush=True)
    with open(f'artifacts/user-latency-{label}.log','w') as log:
        done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
    data=json.load(open(f'artifacts/user-latency-{label}.json'))
    assert done.returncode==0 and data['passed']
    rows=list(csv.DictReader(open(f'artifacts/user-latency-{label}.csv')))
    assert len(rows)==100000 and len({r['id'] for r in rows})==100000
    assert len({(r['room'],r['member']) for r in rows})==100000
    assert all(int(r['samples'])==seconds*rate for r in rows)
    assert all(float(r['p50_ms'])<=float(r['p95_ms'])<=float(r['p99_ms'])<=float(r['max_ms'])<=float(r['schedule_max_ms']) for r in rows)
    print(json.dumps({'passed':label,'users':len(rows),'samples_each':seconds*rate}),flush=True)
