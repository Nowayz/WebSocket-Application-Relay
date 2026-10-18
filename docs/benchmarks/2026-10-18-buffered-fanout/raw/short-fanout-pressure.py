"""Separate verified live slow-reader eviction from the deliberate close burst."""
import hashlib,importlib.util,json,os,select,subprocess,time
from pathlib import Path
root=Path.cwd();prefix=root/'artifacts/short-fanout-pressure'
spec=importlib.util.spec_from_file_location('corpus',root/'scripts/input-corpus.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
port,admin=c.wire.runtime.free_port(),c.wire.runtime.free_port()
serverpath=root/'artifacts/candidates/short-fanout-batch-selected/war-relay';clientpath=root/'artifacts/candidates/mixed-capacity-pressure-client/stress'
command=[str(serverpath),'--bind',f'127.0.0.1:{port}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--messages-per-second','0','--bytes-per-second','0','--ping-interval-secs','0','--idle-timeout-secs','0','--max-message-bytes','1048576','--queue-bytes','1048586','--queue-messages','64','--total-queue-bytes','268435456','--tcp-send-buffer-bytes','16384']
clientcommand=[str(clientpath),'--url',f'ws://127.0.0.1:{port}','--mode','fanout','--clients','1100','--unread-clients','8','--duration-secs','5','--payload-bytes','1024','--window','4','--teardown-gate']
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env['WAR_ADMIN_TOKEN']=c.TOKEN
record={'passed':False,'server_command':command,'client_command':clientcommand,'server_sha256':hashlib.sha256(serverpath.read_bytes()).hexdigest(),'client_sha256':hashlib.sha256(clientpath.read_bytes()).hexdigest(),'events':[]}
with open(str(prefix)+'-server.log','w') as log,open(str(prefix)+'-client.err','w') as errors:
 server=subprocess.Popen(command,env=env,stdout=log,stderr=log);client=None
 try:
  for _ in range(300):
   try:c.metrics(admin);break
   except OSError:time.sleep(.01)
  client=subprocess.Popen(clientcommand,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,text=True)
  for phase in ['ready','traffic_finished','result']:
   assert select.select([client.stdout],[],[],60)[0],phase
   event=json.loads(client.stdout.readline());assert event['event']==phase;record['events'].append(event)
   if phase=='traffic_finished':
    live=c.metrics(admin);record['live_metrics']=live
    assert live['war_slow_consumers_total']==8 and live['war_global_pressure_total']==0,live
    for key in ['war_protocol_errors_total','war_write_errors_total','war_write_timeouts_total','war_routing_copied_bytes_total']:assert live[key]==0,(key,live[key])
    assert event['verified_deliveries']==event['sent_messages']*1091
    client.stdin.write('continue\n');client.stdin.flush()
  record['client_exit_code']=client.wait(timeout=10);assert record['client_exit_code']==0
  result=record['events'][-1];record['result']=result
  assert result['healthy_readers']==1091 and result['verified_deliveries']>0
  assert result['disconnect_notifications']==8*1091
  assert len(result['unread_observations'])==8 and all(x['kind'] in ['close','eof','transport_error'] and (x['kind']!='close' or x['code']==1013) for x in result['unread_observations'])
  for _ in range(1000):
   final=c.metrics(admin)
   if final['war_connections']==0:break
   time.sleep(.01)
  for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']:assert final[key]==0,(key,final[key])
  assert final['war_channels']==1 and 8<=final['war_slow_consumers_total']<=1100
  record['final_metrics']=final;record['teardown_capacity_markings']=final['war_slow_consumers_total']-8
  record['passed']=True
 except Exception as error:
  record['error']=repr(error);raise
 finally:
  if client and client.poll() is None:client.kill();client.wait()
  server.terminate();record['server_exit_code']=server.wait(timeout=10)
  prefix.with_suffix('.json').write_text(json.dumps(record,indent=2)+'\n')
assert record['passed'] and record['server_exit_code']==0
print(json.dumps({'passed':True,'live_slow_consumers':8,'healthy_readers':1091,'verified_deliveries':result['verified_deliveries'],'teardown_capacity_markings':record['teardown_capacity_markings'],'teardown_capacity_closes':result['teardown_capacity_closes']}))
