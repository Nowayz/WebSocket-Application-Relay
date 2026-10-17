import json,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
args=json.loads('{"server": "artifacts/candidates/monoio-completion-bounded/war-relay", "client": "artifacts/baselines/capacity-final/capacity", "server_preload": null, "users": 100000, "room_size": 16, "concurrency": 64, "source_addresses": 64, "seconds": 35, "waves": 1, "payload_bytes": 1024, "active_percent": 100, "client_workers": 8, "messages_per_second": 1, "churn_workers": 16, "ping_interval_secs": 15, "tcp_send_buffer_bytes": 16384, "tcp_receive_buffer_bytes": 65536, "connection_headroom": 10000, "profile": false, "profile_kernel": false, "pressure_client": "artifacts/baselines/capacity-final/stress", "workers": 4, "read_storage": "worker", "server_cpus": "0-3", "client_cpus": "8-15", "timeout": 240, "output": "artifacts/monoio-completion-bounded-mixed.json"}')
for name,server in [('mio','artifacts/candidates/input-batch/war-relay'),('timer','artifacts/candidates/timer-inplace/war-relay')]:
    values={**args,'server':server,'output':f'artifacts/timer-inplace-mixed-{name}.json'}
    command=[sys.executable,'scripts/capacity.py']
    for key,value in values.items():
        if value is None or value is False: continue
        command.append('--'+key.replace('_','-'))
        if value is not True: command.append(str(value))
    print(json.dumps({'started':name,'command':command}),flush=True)
    with open(f'artifacts/timer-inplace-mixed-{name}.log','w') as log:
        result=subprocess.run(command,cwd=ROOT,stdout=log,stderr=log,timeout=240)
    data=json.loads((ROOT/values['output']).read_text())
    assert result.returncode==0 and data['passed'],name
    print(json.dumps({'passed':name,'measurements':data['measurements']}),flush=True)
