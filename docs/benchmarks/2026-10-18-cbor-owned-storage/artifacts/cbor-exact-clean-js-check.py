import hashlib,importlib.util,json,os,socket,subprocess,time,urllib.request
from pathlib import Path
root=Path.cwd()
def free():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
ports=set()
while len(ports)<3:ports.add(free())
native,admin,bridge=ports
server=root/'artifacts/candidates/mixed-capacity-final/war-relay'
proxy=root/'artifacts/candidates/cbor-exact-bridge/war-legacy-bridge'
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_AUTH_PASSWORD='js-test-host',WAR_ADMIN_TOKEN='bridge-encoding-check',WAR_TEST_URL=f'ws://127.0.0.1:{native}',WAR_TEST_BRIDGE_URL=f'ws://127.0.0.1:{bridge}')
commands=[[str(server),'--bind',f'127.0.0.1:{native}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--idle-timeout-secs','0'],[str(proxy),'--bind',f'127.0.0.1:{bridge}','--upstream',f'ws://127.0.0.1:{native}']]
record={'passed':False,'commands':commands,'server_sha256':hashlib.sha256(server.read_bytes()).hexdigest(),'bridge_sha256':hashlib.sha256(proxy.read_bytes()).hexdigest()};processes=[];logs=[]
try:
 for name,cmd in zip(['native','bridge'],commands):
  log=open(root/f'artifacts/cbor-exact-clean-js-{name}.log','w');logs.append(log);processes.append(subprocess.Popen(cmd,env=env,stdout=log,stderr=log))
 for _ in range(300):
  assert all(p.poll() is None for p in processes)
  try:
   with urllib.request.urlopen(f'http://127.0.0.1:{admin}/readyz',timeout=.2) as r:assert r.status==200
   with socket.create_connection(('127.0.0.1',bridge),timeout=.2):pass
   break
  except OSError:time.sleep(.01)
 cmd=['node','--test','--test-timeout=10000','clients/js/relay.test.mjs']
 result=subprocess.run(cmd,env=env,capture_output=True,text=True,timeout=45);record.update(test_command=cmd,test_exit_code=result.returncode,test_output=result.stdout+result.stderr)
 assert result.returncode==0,record['test_output']
 for _ in range(300):
  req=urllib.request.Request(f'http://127.0.0.1:{admin}/metrics',headers={'Authorization':'Bearer bridge-encoding-check'})
  with urllib.request.urlopen(req,timeout=2) as r:metrics=dict((k,int(v)) for k,v in (s.split() for s in r.read().decode().splitlines() if s.startswith('war_')))
  if metrics['war_connections']==metrics['war_sessions']==metrics['war_queued_bytes']==0:break
  time.sleep(.01)
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes']:assert metrics[key]==0,(key,metrics[key])
 record.update(passed=True,final_resource_gauges={k:metrics[k] for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes']})
finally:
 for p in reversed(processes):
  if p.poll() is None:p.terminate()
  p.wait(timeout=10)
 record['exit_codes']=[p.returncode for p in processes]
 for log in logs:log.close()
 (root/'artifacts/cbor-exact-clean-js-check.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
