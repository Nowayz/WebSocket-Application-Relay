import importlib.util,json,os,subprocess,time
from pathlib import Path
ROOT=Path.cwd();spec=importlib.util.spec_from_file_location('corpus',ROOT/'scripts/input-corpus.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
port,admin=c.wire.runtime.free_port(),c.wire.runtime.free_port()
cmd=[str(ROOT/'artifacts/candidates/mixed-capacity-final/war-relay'),'--bind',f'127.0.0.1:{port}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--messages-per-second','0','--bytes-per-second','0','--ping-interval-secs','0','--idle-timeout-secs','0','--max-message-bytes','1048576','--queue-bytes','1048586','--queue-messages','64','--total-queue-bytes','268435456','--tcp-send-buffer-bytes','16384']
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env['WAR_ADMIN_TOKEN']=c.TOKEN
record={'server_command':cmd,'passed':False}
with open(ROOT/'artifacts/mixed100k-pressure-large-final-server.log','w') as log:
 server=subprocess.Popen(cmd,env=env,stdout=log,stderr=log)
 try:
  for _ in range(200):
   try:c.metrics(admin);break
   except OSError:time.sleep(.01)
  command=[str(ROOT/'artifacts/baselines/capacity-final/stress'),'--url',f'ws://127.0.0.1:{port}','--mode','fanout','--clients','1030','--unread-clients','8','--duration-secs','5','--payload-bytes','1024','--window','1']
  result=subprocess.run(command,capture_output=True,text=True,timeout=40);record.update(client_command=command,client_log=result.stdout+result.stderr,client_exit_code=result.returncode)
  assert result.returncode==0,record['client_log']
  events=[json.loads(s) for s in result.stdout.splitlines() if s.startswith('{')];r=next(e for e in events if e.get('event')=='result');record['result']=r
  assert r['verified_deliveries']>0 and r['disconnect_notifications']==8*1021,r
  assert len(r['unread_observations'])==8 and all(x['kind'] in ['close','eof','transport_error'] and (x['kind']!='close' or x['code']==1013) for x in r['unread_observations']),r
  for _ in range(500):
   final=c.metrics(admin)
   if final['war_connections']==final['war_queued_bytes']==0:break
   time.sleep(.01)
  for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']:assert final[key]==0,(key,final[key])
  assert final['war_slow_consumers_total']==8,final
  record.update(passed=True,final_metrics=final)
 finally:
  server.terminate();record['server_exit_code']=server.wait(timeout=10)
  (ROOT/'artifacts/mixed100k-pressure-large-final.json').write_text(json.dumps(record,indent=2)+'\n')
assert record['passed'] and record['server_exit_code']==0
print(json.dumps(record['result']))
