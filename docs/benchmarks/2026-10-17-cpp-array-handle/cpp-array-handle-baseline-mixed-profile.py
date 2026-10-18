from pathlib import Path
import hashlib,json,os,socket,subprocess,time
root=Path.cwd()
with socket.socket() as reserve:
 reserve.bind(('127.0.0.1',0));port=reserve.getsockname()[1]
server_path=root/'artifacts/candidates/cpp-array-handle-control/war-relay-cpp'
client_path=root/'artifacts/candidates/observer-quantum-client/inspection'
prefix=root/'artifacts/cpp-array-handle-baseline-mixed-profile'
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
env['WAR_ADMIN_TOKEN']='cpp-comparison-fixture'
env['WAR_AUTH_PASSWORD']='inspection-benchmark'
command=['taskset','-c','0-3',str(server_path),'--bind',f'127.0.0.1:{port}','--workers','4','--pin-workers','--no-admin']
client=['taskset','-c','8-15',str(client_path),'--url',f'ws://127.0.0.1:{port}','--protocol-only','--mode','cbor','--rooms','64','--seconds','6','--inspectors','2','--pairs','1','--churn-workers','8','--client-workers','8','--fanout-readers','512','--fanout-global-observers','--fanout-window','16']
record={'scope':'Separate diagnostic cycles:user sample on accepted C++ under mixed load; not ordinary timing or an off-CPU/wait-time profile.','server_command':command,'client_command':client,'server_sha256':hashlib.sha256(server_path.read_bytes()).hexdigest(),'client_sha256':hashlib.sha256(client_path.read_bytes()).hexdigest(),'passed':False}
with open(str(prefix)+'-server.log','w') as output:
 server=subprocess.Popen(command,env=env,stdout=output,stderr=output)
 profiler=None
 try:
  time.sleep(.2);assert server.poll() is None
  profiler=subprocess.Popen(['perf','record','-e','cycles:u','-F','199','--call-graph','dwarf,8192','-o',str(prefix)+'.perf.data','-p',str(server.pid),'--','sleep','7'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  result=subprocess.run(client,input='continue\n'*8,capture_output=True,text=True,timeout=30)
  Path(str(prefix)+'-client.log').write_text(result.stdout+result.stderr)
  stdout,stderr=profiler.communicate(timeout=15);Path(str(prefix)+'-perf.log').write_bytes(stdout+stderr)
  assert profiler.returncode==result.returncode==0,(profiler.returncode,result.returncode,result.stderr)
  events=[json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')];record['events']=events
  record['passed']=any(e.get('event')=='done' for e in events)
 finally:
  if profiler and profiler.poll() is None:profiler.terminate();profiler.wait(timeout=10)
  server.terminate();record['server_exit_code']=server.wait(timeout=10)
Path(str(prefix)+'.json').write_text(json.dumps(record,indent=2)+'\n')
assert record['passed'] and record['server_exit_code']==0
subprocess.run(['perf','report','-i',str(prefix)+'.perf.data','--stdio','--no-children','--sort','symbol','--percent-limit','1','--call-graph','none'],stdout=open(str(prefix)+'.txt','w'),check=True)
