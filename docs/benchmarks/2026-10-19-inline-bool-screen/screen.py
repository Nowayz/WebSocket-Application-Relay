import sys,os,time,json,subprocess,tempfile,signal,hashlib
from pathlib import Path
sys.path.insert(0,str(Path.cwd()/'scripts'))
import benchmark
root=Path.cwd();sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest();rows=[]
client=root/'artifacts/candidates/native-inline-bool-client-v2/bool_commands'
for index,variant in enumerate(['control','candidate','candidate','control']):
 binary=root/('artifacts/candidates/native-inline-u64/war-relay' if variant=='control' else 'artifacts/candidates/native-inline-bool-candidate/war-relay')
 port,admin=benchmark.free_port(),benchmark.free_port();assert port!=admin
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env['WAR_AUTH_PASSWORD']='bool-output-check'
 cmd=['taskset','-c','0-3',str(binary),'--bind',f'127.0.0.1:{port}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--pin-workers','--messages-per-second','0','--bytes-per-second','0','--ping-interval-secs','0','--idle-timeout-secs','0']
 row={'variant':variant,'server_command':cmd,'server_sha256':sha(binary),'client_sha256':sha(client),'passed':False}
 with tempfile.TemporaryFile() as log:
  server=subprocess.Popen(cmd,env=env,stdout=log,stderr=log)
  try:
   for _ in range(500):
    assert server.poll() is None
    try:benchmark.metrics(admin);break
    except OSError:time.sleep(.01)
   else:raise TimeoutError('readiness')
   before=benchmark.snapshot(server.pid);start=time.monotonic()
   client_cmd=['taskset','-c','4-7,20-23',str(client),f'ws://127.0.0.1:{port}'];run=subprocess.run(client_cmd,capture_output=True,text=True,timeout=30)
   elapsed=time.monotonic()-start;after=benchmark.snapshot(server.pid)
   row.update(client_command=client_cmd,client_exit_code=run.returncode,client_output=run.stdout+run.stderr,before=before,after=after,elapsed_seconds=elapsed)
   assert run.returncode==0,row['client_output'];assert run.stdout.strip()=='verified 100000 boolean acknowledgements'
   for _ in range(300):
    m={k:int(v) for k,v in benchmark.metrics(admin).items()}
    if m['war_connections']==m['war_sessions']==0:break
    time.sleep(.01)
   assert m['war_incoming_messages_total']==100000 and m['war_deliveries_total']==100008
   assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==8
   for k in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_variable_bytes']:assert m[k]==0
   for k,v in m.items():
    if any(w in k for w in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','close_failures_total','transport_disconnects_total','global_pressure_total']):assert v==0,(k,v)
   row.update(passed=True,metrics=m,cpu_seconds=after['cpu_seconds']-before['cpu_seconds'])
  finally:
   if server.poll() is None:server.send_signal(signal.SIGINT)
   row['server_exit_code']=server.wait(timeout=10);log.seek(0);row['server_log']=log.read().decode()
   path=root/f'artifacts/native-inline-bool-{index}-{variant}.json';assert not path.exists();path.write_text(json.dumps(row,indent=2)+'\n')
 assert row['server_exit_code']==0
 rows.append(row);print(index,variant,row['cpu_seconds'],elapsed,flush=True)
(root/'artifacts/native-inline-bool-results.json').write_text(json.dumps(rows,indent=2)+'\n')
