import hashlib,json,re,subprocess,sys
from pathlib import Path
server=Path('artifacts/candidates/timer-attribution-20261019-diagnostic/war-relay')
client=Path('artifacts/candidates/latency-timeline-client/capacity')
result=Path('artifacts/timer-attribution-20261019-summary.json')
assert not result.exists()
record={'diagnostic_only':True,'passed':False,'runs':[]}
for label,users,ping,workers in [('disabled',8192,0,4),('normal',8192,15,4),('frequent',8192,1,4),('target',100000,15,8)]:
 prefix=f'artifacts/timer-attribution-20261019-{label}'
 cmd=[sys.executable,'scripts/capacity.py','--server',str(server),'--client',str(client),'--users',str(users),'--seconds','2','--messages-per-second','4','--workers',str(workers),'--client-workers','8','--server-cpus',('0-7' if workers==8 else '0-3'),'--client-cpus','8-15','--pin-workers','--pin-client-workers','--ping-interval-secs',str(ping),'--tcp-send-buffer-bytes','16384','--tcp-receive-buffer-bytes','65536','--output',prefix+'.json','--timeout','60']
 print(json.dumps({'started':label,'command':cmd}),flush=True)
 with open(prefix+'.log','x') as logs: proc=subprocess.run(cmd,stdout=logs,stderr=logs,timeout=90)
 data=json.loads(Path(prefix+'.json').read_text())
 assert proc.returncode==0 and data['passed'],label
 assert data['server_exit_code']==data['client_exit_code']==0
 assert data['server_sha256']==hashlib.sha256(server.read_bytes()).hexdigest()
 counts=[(json.loads(m[0]),int(m[1]),int(m[2])) for m in re.findall(r'timer-attribution: (\[[^\n]+?\]) peak=(\d+) remaining=(\d+)',data['server_log'])]
 assert counts and all(remaining==0 for _,_,remaining in counts)
 totals=[sum(c[i] for c,_,_ in counts) for i in range(6)]
 assert sum(totals)>0
 done=next(e for e in data['events'] if e['event']=='done')
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
  assert done['metrics'][key]==0,key
 assert done['server']['fd_count']==data['baseline']['server']['fd_count']
 assert data['measurement']['application_deliveries']==users*8
 row={'label':label,'users':users,'ping_interval_secs':ping,'command':cmd,'receipt':prefix+'.json','receipt_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'categories':['absent_to_absent','live_to_absent','insert','unchanged','earlier','later'],'counts':totals,'setter_calls':sum(totals),'unchanged_percent':totals[3]/sum(totals)*100,'diagnostic_instances':len(counts),'peak_timers_per_instance':[peak for _,peak,_ in counts]}
 record['runs'].append(row);result.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;result.write_text(json.dumps(record,indent=2)+'\n')
