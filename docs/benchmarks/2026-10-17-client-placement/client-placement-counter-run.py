import json,subprocess,sys
from pathlib import Path
prefix='artifacts/client-placement-counter'
assert not Path(prefix+'.json').exists()
command=['sudo','-n','prlimit','--nofile=1048576:1048576','--memlock=unlimited','--',sys.executable,'scripts/capacity.py','--server','artifacts/candidates/heartbeat-phase/war-relay','--client','artifacts/candidates/latency-timeline-client/capacity','--users','100000','--seconds','30','--messages-per-second','6','--workers','8','--client-workers','8','--server-cpus','0-7','--client-cpus','8-15','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--ping-interval-secs','15','--pin-workers','--profile','--profile-client','--profile-counters','--user-latency-output',prefix+'.csv','--output',prefix+'.json']
print(json.dumps({'command':command}),flush=True)
result=subprocess.run(command)
x=json.loads(Path(prefix+'.json').read_text());assert result.returncode==0 and x['passed'] and x['client_exit_code']==x['server_exit_code']==0
