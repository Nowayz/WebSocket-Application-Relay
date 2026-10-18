import hashlib,json,os,select,signal,socket,subprocess,time
from pathlib import Path
root=Path.cwd();prefix=root/'artifacts/rust-owner-fanout-explicit-profile'
ports=[]
for _ in range(2):
 with socket.socket() as s:s.bind(('127.0.0.1',0));ports.append(s.getsockname()[1])
serverpath=root/'artifacts/candidates/rust-owner-fanout-explicit/war-relay';clientpath=root/'artifacts/baselines/active-pairs-client/inspection'
cmd=['taskset','-c','0-3',str(serverpath),'--bind',f'127.0.0.1:{ports[0]}','--admin-bind',f'127.0.0.1:{ports[1]}','--workers','4','--messages-per-second','0','--bytes-per-second','0','--ping-interval-secs','0','--idle-timeout-secs','0']
cc=['taskset','-c','4-11',str(clientpath),'--url',f'ws://127.0.0.1:{ports[0]}','--wire','cbor','--protocol-only','--mode','none','--rooms','8192','--seconds','8','--pairs','0','--churn-workers','0','--client-workers','8','--fanout-readers','63']
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_AUTH_PASSWORD='inspection-benchmark',WAR_ADMIN_TOKEN='inspection-benchmark')
record={'diagnostic_only':True,'server_command':cmd,'client_command':cc,'server_sha256':hashlib.sha256(serverpath.read_bytes()).hexdigest(),'events':[]}
with open(str(prefix)+'-server.log','w') as log,open(str(prefix)+'-client-errors.log','w') as errors:
 server=subprocess.Popen(cmd,env=env,stdout=log,stderr=log);client=None;prof=None
 try:
  time.sleep(.2)
  client=subprocess.Popen(cc,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,text=True,env=env)
  for phase in ['ready','measured','done']:
   assert select.select([client.stdout],[],[],40)[0],phase
   e=json.loads(client.stdout.readline());assert e['event']==phase;e['server_threads']={t.name:Path(t/'stat').read_text() for t in Path(f'/proc/{server.pid}/task').iterdir()};record['events'].append(e)
   if phase=='ready':
    prof=subprocess.Popen(['perf','record','-e','cycles:u','-F','499','--call-graph','dwarf,8192','-o',str(prefix)+'.perf.data','-p',str(server.pid)],stdout=subprocess.PIPE,stderr=subprocess.PIPE);time.sleep(.15)
   if phase=='measured':
    prof.send_signal(signal.SIGINT);a,b=prof.communicate(timeout=10);Path(str(prefix)+'-perf.log').write_bytes(a+b);assert prof.returncode in (0,130,-signal.SIGINT)
   client.stdin.write('continue\n');client.stdin.flush()
  assert client.wait(timeout=5)==0;record['passed']=True
 finally:
  if client and client.poll() is None:client.kill();client.wait()
  if prof and prof.poll() is None:prof.terminate();prof.wait()
  server.terminate();record['server_exit_code']=server.wait(timeout=10)
  Path(str(prefix)+'.json').write_text(json.dumps(record,indent=2)+'\n')
subprocess.run(['perf','report','-i',str(prefix)+'.perf.data','--stdio','--no-children','--sort','symbol','--percent-limit','1','--call-graph','none'],stdout=open(str(prefix)+'.txt','w'),check=True)
