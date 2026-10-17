"""Sequential ordinary 100k capacity comparisons; binaries are frozen first."""
import json,subprocess,sys
from pathlib import Path
root=Path(__file__).resolve().parents[1]
candidates=[('control',root/'artifacts/baselines/capacity-final/war-relay'),('batch',root/'artifacts/baselines/c10m-batch/war-relay')]
for repetition in range(3):
    order=candidates[repetition%2:]+candidates[:repetition%2]
    for label,binary in order:
        out=root/f'artifacts/c10m-batch-overload-{repetition}-{label}.json'
        command=[sys.executable,str(root/'scripts/capacity.py'),'--server',str(binary),'--client',str(root/'artifacts/baselines/capacity-final/capacity'),'--users','100000','--seconds','5','--messages-per-second','10','--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--output',str(out)]
        with out.with_suffix('.log').open('w') as log:
            subprocess.run(command,check=True,stdout=log,stderr=subprocess.STDOUT)
        data=json.loads(out.read_text())
        measurement=data['measurements'][0]
        event=data['events'][2]['details']
        print(json.dumps({'repetition':repetition,'candidate':label,'passed':data['passed'],'rate':measurement['stable_deliveries_per_second'],'cpu_ns_per_delivery':measurement['server_cpu_seconds']*1e9/event['round_trip']['samples'],'user_seconds':measurement['server_user_cpu_seconds'],'system_seconds':measurement['server_system_cpu_seconds'],'rtt':event['round_trip']}),flush=True)
