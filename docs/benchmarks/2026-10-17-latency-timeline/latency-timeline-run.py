import csv,json,subprocess,sys,hashlib
from pathlib import Path
root=Path.cwd()
for label,ping in [('on-1',15),('off-1',0),('off-2',0),('on-2',15)]:
    seconds,rate,users=30,4,100000
    prefix=f'artifacts/latency-timeline-{label}'
    assert not Path(prefix+'.json').exists()
    command=[sys.executable,'scripts/capacity.py','--server','artifacts/candidates/input-batch/war-relay','--client','artifacts/candidates/latency-timeline-client/capacity','--users',str(users),'--seconds',str(seconds),'--messages-per-second',str(rate),'--read-storage','worker','--server-cpus','0-3','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs',str(ping),'--user-latency-output',prefix+'.csv','--output',prefix+'.json']
    print(json.dumps({'started':label,'command':command}),flush=True)
    with open(prefix+'.log','w') as log:done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
    data=json.load(open(prefix+'.json'));assert done.returncode==0 and data['passed']
    rows=list(csv.DictReader(open(prefix+'.csv')))
    assert len(rows)==users and len({r['id'] for r in rows})==users
    assert all(int(r['samples'])==seconds*rate for r in rows)
    timeline=json.load(open(prefix+'.windows.json'))['windows']
    assert len(timeline)==seconds
    assert sum(w['send_to_receive']['samples'] for w in timeline)==users*seconds*rate
    assert all(w['send_to_receive']['samples']==users*rate for w in timeline)
    assert all(w['pre_send_delay']['samples']==w['intended_to_receive']['samples']==w['send_to_receive']['samples'] for w in timeline)
    pings=sum(w['pings_encountered'] for w in timeline)
    assert (pings>0)==bool(ping)
    values=sorted(float(r['p99_ms']) for r in rows)
    result={'passed':label,'users':users,'samples_each':seconds*rate,'pings_encountered':pings,'median_user_p99_ms':values[users//2-1],'p99_user_p99_ms':values[(users*99+99)//100-1],'worst_packet_ms':max(float(r['max_ms']) for r in rows),'timeline':timeline}
    Path(prefix+'-summary.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({k:v for k,v in result.items() if k!='timeline'}),flush=True)
