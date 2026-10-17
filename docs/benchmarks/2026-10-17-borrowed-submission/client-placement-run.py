import csv,json,subprocess,sys,hashlib
from pathlib import Path
root=Path.cwd()
record={'scope':'Client placement ablation at600k messages/s,100000users1KiB private,30seconds15sheartbeat. Server8workers always pinned0-7;client8workers allowed8-15,unbound versus each singleton. Same server/client executable, buffers, workload. Fresh unbound/pinned/pinned/unbound processes, no profiling or competing load. Diagnostic generator intervention, not a relay speedup.','runs':[],'passed':False}
for repetition,names in [(1,['unbound','pinned']),(2,['pinned','unbound'])]:
    for name in names:
        prefix=f'artifacts/client-placement-{name}-{repetition}'
        assert not Path(prefix+'.json').exists(),prefix
        command=[sys.executable,'scripts/capacity.py','--server','artifacts/candidates/worker-affinity/war-relay','--client','artifacts/candidates/latency-timeline-client/capacity','--users','100000','--seconds','30','--messages-per-second','6','--workers','8','--client-workers','8','--read-storage','worker','--server-cpus','0-7','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--user-latency-output',prefix+'.csv','--output',prefix+'.json']
        command.append('--pin-workers')
        if name=='pinned':command.append('--pin-client-workers')
        print(json.dumps({'started':name,'repetition':repetition,'command':command}),flush=True)
        with open(prefix+'.log','w') as log:done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
        data=json.load(open(prefix+'.json'));assert done.returncode==0 and data['passed']
        assert data['server_exit_code']==data['client_exit_code']==0 and not data.get('server_forced_kill')
        expected=16 if name=='pinned' else 8
        assert len(data['thread_affinity'])==expected and all(x['verified_after_measurement'] for x in data['thread_affinity'])
        assert data['server_sha256']==hashlib.sha256(Path(command[3]).read_bytes()).hexdigest()
        rows=list(csv.DictReader(open(prefix+'.csv')))
        assert len(rows)==100000 and len({r['id'] for r in rows})==100000 and all(int(r['samples'])==180 for r in rows)
        values=sorted(float(r['p99_ms']) for r in rows)
        row={'kind':name,'repetition':repetition,'trace':prefix+'.json','trace_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'users':len(rows),'samples_each':180,'median_user_p99_ms':values[49999],'p95_user_p99_ms':values[94999],'p99_user_p99_ms':values[98999],'worst_user_p99_ms':values[-1],'worst_packet_ms':max(float(r['max_ms']) for r in rows),'measurement':data['measurement']}
        scheduled=sorted(float(r['schedule_p99_ms']) for r in rows)
        row['median_user_intended_p99_ms']=scheduled[49999]
        row['p99_user_intended_p99_ms']=scheduled[98999]
        row['worst_intended_packet_ms']=max(float(r['schedule_max_ms']) for r in rows)
        record['runs'].append(row)
        Path('artifacts/client-placement-summary.json').write_text(json.dumps(record,indent=2)+'\n')
        print(json.dumps({k:v for k,v in row.items() if k!='measurement'}),flush=True)
record['passed']=True
Path('artifacts/client-placement-summary.json').write_text(json.dumps(record,indent=2)+'\n')
