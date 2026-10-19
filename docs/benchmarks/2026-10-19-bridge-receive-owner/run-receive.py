import sys,subprocess,tempfile,os,json,time,signal,hashlib
from pathlib import Path
sys.path.insert(0,str(Path.cwd()/'scripts'));import benchmark
server=Path('artifacts/candidates/worker-count-20261019-current/war-relay').resolve();client=Path('artifacts/candidates/bridge-receive-owner-20261019/receive_owner').resolve();output=Path('artifacts/bridge-receive-owner-20261019.json');assert not output.exists()
port,admin=benchmark.free_port(),benchmark.free_port();assert port!=admin
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
command=[str(server),'--bind',f'127.0.0.1:{port}','--admin-bind',f'127.0.0.1:{admin}','--workers','1','--max-message-bytes','1048576','--queue-bytes','2097152','--ping-interval-secs','0','--idle-timeout-secs','0','--messages-per-second','0','--bytes-per-second','0']
r={'passed':False,'server_command':command,'server_sha256':hashlib.sha256(server.read_bytes()).hexdigest(),'client_sha256':hashlib.sha256(client.read_bytes()).hexdigest()}
with tempfile.TemporaryFile() as log:
 p=subprocess.Popen(command,env=env,stdout=log,stderr=log)
 try:
  for _ in range(300):
   assert p.poll() is None
   try:benchmark.metrics(admin);break
   except OSError:time.sleep(.01)
  else:raise TimeoutError('readiness')
  cmd=[str(client),f'ws://127.0.0.1:{port}'];start=time.monotonic();result=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=10)
  r.update(client_command=cmd,client_exit_code=result.returncode,client_output=result.stdout+result.stderr,elapsed_seconds=time.monotonic()-start);assert result.returncode==0,r['client_output'];r['observations']=[json.loads(s) for s in result.stdout.splitlines()]
  for _ in range(300):
   metrics={k:int(v) for k,v in benchmark.metrics(admin).items()}
   if metrics['war_connections']==metrics['war_sessions']==0:break
   time.sleep(.01)
  assert metrics['war_incoming_messages_total']==6 and metrics['war_deliveries_total']==7
  assert metrics['war_accepted_total']==metrics['war_joined_total']==metrics['war_disconnected_total']==1
  for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:assert metrics[k]==0
  r.update(passed=True,final_metrics=metrics)
 finally:
  if p.poll() is None:p.send_signal(signal.SIGINT)
  r['server_exit_code']=p.wait(timeout=10);log.seek(0);r['server_log']=log.read().decode();output.write_text(json.dumps(r,indent=2)+'\n')
print(json.dumps({'passed':r['passed'],'elapsed_seconds':r.get('elapsed_seconds'),'observations':r.get('observations'),'server_exit_code':r['server_exit_code']}))
