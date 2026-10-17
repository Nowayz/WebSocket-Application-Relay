import json,subprocess,sys,hashlib
from pathlib import Path
record={'passed':False,'scope':'Secondary peak-throughput tradeoff: same new native executable, pin-workers off/on in three rotating pairs;100000 users,1KiB,one million offered messages/s for five seconds plus drain,server0-3/client8-15,16KiB/64KiB TCP,heartbeat disabled. Frozen original verifying capacity client. No builds/profiles/load overlap. Not a low-latency operating rate.','runs':[]}
sha=hashlib.sha256(Path('artifacts/candidates/worker-affinity/war-relay').read_bytes()).hexdigest()
for repetition in range(1,4):
    order=['none','native'] if repetition%2 else ['native','none']
    for kind in order:
        prefix=f'artifacts/worker-affinity-throughput-{kind}-{repetition}'
        assert not Path(prefix+'.json').exists()
        command=[sys.executable,'scripts/capacity.py','--server','artifacts/candidates/worker-affinity/war-relay','--client','artifacts/baselines/capacity-final/capacity','--users','100000','--seconds','5','--messages-per-second','10','--workers','4','--client-workers','8','--read-storage','worker','--server-cpus','0-3','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--output',prefix+'.json']
        if kind=='native':command.append('--pin-workers')
        print(json.dumps({'started':kind,'repetition':repetition}),flush=True)
        with open(prefix+'.log','w') as log:done=subprocess.run(command,stdout=log,stderr=log,timeout=240)
        data=json.load(open(prefix+'.json'));assert done.returncode==0 and data['passed'] and data['server_sha256']==sha
        assert data['server_exit_code']==data['client_exit_code']==0
        assert len(data['thread_affinity'])==(4 if kind=='native' else 0)
        assert all(t['verified_after_measurement'] for t in data['thread_affinity'])
        measurement=data['measurement'];assert measurement['application_deliveries']==5000000
        row={'kind':kind,'repetition':repetition,'trace':prefix+'.json','trace_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'deliveries_per_second':measurement['stable_deliveries_per_second'],'cpu_ns_per_delivery':measurement['server_cpu_seconds']/5000000*1e9,'p99_upper_ms':measurement['round_trip']['p99_upper_ms'],'ready_rss_mib':data['events'][1]['server']['VmRSS']/1024}
        record['runs'].append(row);Path('artifacts/worker-affinity-throughput-summary.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True
Path('artifacts/worker-affinity-throughput-summary.json').write_text(json.dumps(record,indent=2)+'\n')
