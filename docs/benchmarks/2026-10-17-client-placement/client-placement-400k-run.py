import csv,json,subprocess,sys,hashlib
from pathlib import Path
root=Path.cwd()
record={'scope':'Unchanged selected heartbeat-phase relay and frozen exact client. Only client worker placement changes: inherited CPU mask8-15 versus one singleton CPU per runtime worker applied at ready gate. Native8workers always pinned0-7.100000users,400000messages/s,30seconds,1KiBprivate,room16,15sheartbeat16/64KiBTCP. Fresh off/on/on/off ordinary processes,no perf/build/other benchmark. Diagnostic/client-placement result,not a relay runtime speedup.','runs':[],'passed':False}
for repetition,names in [(1,['baseline','pinned']),(2,['pinned','baseline'])]:
    for name in names:
        prefix=f'artifacts/client-placement-400k-{name}-{repetition}'
        assert not Path(prefix+'.json').exists(),prefix
        command=[sys.executable,'scripts/capacity.py','--server','artifacts/candidates/heartbeat-phase/war-relay','--client','artifacts/candidates/latency-timeline-client/capacity','--users','100000','--seconds','30','--messages-per-second','4','--workers','8','--client-workers','8','--read-storage','worker','--server-cpus','0-7','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--user-latency-output',prefix+'.csv','--output',prefix+'.json']
        command.append('--pin-workers')
        if name=='pinned': command.append('--pin-client-workers')
        print(json.dumps({'started':name,'repetition':repetition,'command':command}),flush=True)
        with open(prefix+'.log','w') as log:done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
        data=json.load(open(prefix+'.json'));assert done.returncode==0 and data['passed']
        assert data['server_exit_code']==data['client_exit_code']==0 and not data.get('server_forced_kill')
        expected=8 if name=='baseline' else 16
        assert len(data['thread_affinity'])==expected and all(x['verified_after_measurement'] for x in data['thread_affinity'])
        assert data['server_sha256']==hashlib.sha256(Path(command[3]).read_bytes()).hexdigest()
        rows=list(csv.DictReader(open(prefix+'.csv')))
        assert len(rows)==100000 and len({r['id'] for r in rows})==100000 and all(int(r['samples'])==120 for r in rows)
        values=sorted(float(r['p99_ms']) for r in rows)
        row={'kind':name,'repetition':repetition,'trace':prefix+'.json','trace_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'users':len(rows),'samples_each':120,'median_user_p99_ms':values[49999],'p95_user_p99_ms':values[94999],'p99_user_p99_ms':values[98999],'worst_user_p99_ms':values[-1],'worst_packet_ms':max(float(r['max_ms']) for r in rows),'measurement':data['measurement']}
        for column in ['schedule_p99_ms', 'schedule_max_ms']:
            ordered=sorted(float(r[column]) for r in rows)
            row[column]={'median':ordered[49999],'p95':ordered[94999],'p99':ordered[98999],'maximum':ordered[-1]}
        record['runs'].append(row)
        Path('artifacts/client-placement-400k-summary.json').write_text(json.dumps(record,indent=2)+'\n')
        print(json.dumps({k:v for k,v in row.items() if k!='measurement'}),flush=True)
record['passed']=True
Path('artifacts/client-placement-400k-summary.json').write_text(json.dumps(record,indent=2)+'\n')
