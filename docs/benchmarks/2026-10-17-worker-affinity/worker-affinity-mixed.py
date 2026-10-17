import json,subprocess,sys
args=json.loads('{"server": "artifacts/candidates/worker-affinity/war-relay", "client": "artifacts/baselines/capacity-final/capacity", "server_preload": null, "users": 100000, "room_size": 16, "concurrency": 64, "source_addresses": 64, "seconds": 65, "waves": 1, "payload_bytes": 1024, "active_percent": 100, "client_workers": 8, "messages_per_second": 1, "churn_workers": 16, "ping_interval_secs": 15, "tcp_send_buffer_bytes": 16384, "tcp_receive_buffer_bytes": 65536, "connection_headroom": 10000, "pin_workers": true, "profile": false, "profile_kernel": false, "pressure_client": "artifacts/baselines/capacity-final/stress", "workers": 4, "read_storage": "worker", "server_cpus": "0-3", "client_cpus": "8-15", "timeout": 240, "output": "artifacts/worker-affinity-mixed.json"}')
command=[sys.executable,'scripts/capacity.py']
for key,value in args.items():
    if value is None or value is False: continue
    command.append('--'+key.replace('_','-'))
    if value is not True:command.append(str(value))
print(json.dumps({'command':command}),flush=True)
result=subprocess.run(command)
sys.exit(result.returncode)
