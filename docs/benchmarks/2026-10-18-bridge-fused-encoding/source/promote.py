"""Guarded, reversible promotion of the tested local bridge; no live state allowed."""
import hashlib,importlib.util,json,os,signal,subprocess,time,urllib.request
from pathlib import Path
root=Path.cwd();metadata=root/'artifacts/reactor-manual-services.json';original=json.loads(metadata.read_text());old=original['pids'][1]
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
def stopped(pid):
 try:return 'State:\tZ' in Path(f'/proc/{pid}/status').read_text()
 except FileNotFoundError:return True
def save(value):
 temporary=metadata.with_suffix('.json.new');temporary.write_text(json.dumps(value,indent=2)+'\n');temporary.replace(metadata)
assert sha(f'/proc/{old}/exe')==original['bridge_sha256']
assert sha(f'/proc/{original["pids"][0]}/exe')==original['native_sha256']
assert not subprocess.check_output(['ss','-Htn','state','established','(','sport','=',':1337',')'],text=True).strip(),'bridge has live clients'
native_env=dict(v.decode(errors='surrogateescape').split('=',1) for v in Path(f'/proc/{original["pids"][0]}/environ').read_bytes().split(b'\0') if b'=' in v)
env=dict(v.decode(errors='surrogateescape').split('=',1) for v in Path(f'/proc/{old}/environ').read_bytes().split(b'\0') if b'=' in v)
def metrics():
 req=urllib.request.Request('http://127.0.0.1:1339/metrics',headers={'Authorization':'Bearer '+native_env.get('WAR_ADMIN_TOKEN','')})
 with urllib.request.urlopen(req,timeout=2) as response:return dict((k,int(v)) for k,v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_')))
keys=['war_connections','war_sessions','war_queued_bytes','war_variable_bytes']
assert all(metrics()[key]==0 for key in keys),'manual native service has live state'
exe=root/'artifacts/candidates/bridge-fused-selected/war-legacy-bridge';build=json.loads(exe.with_name('build.json').read_text());assert sha(exe)==build['binary_sha256']
record={'passed':False,'old_bridge_pid':old,'old_bridge_sha256':original['bridge_sha256'],'bridge_sha256':sha(exe),'source_revision':build['source_revision'],'native_pid_unchanged':original['pids'][0],'native_sha256_unchanged':sha(f'/proc/{original["pids"][0]}/exe'),'static_pid_unchanged':original['pids'][2]}
command=list(original['commands'][1]);command[0]=str(exe);new=None
try:
 os.kill(old,signal.SIGTERM)
 for _ in range(500):
  if stopped(old):break
  time.sleep(.01)
 assert stopped(old),'original bridge has not stopped; do not start a duplicate'
 with open(root/'artifacts/bridge-fused-manual.log','ab') as log:new=subprocess.Popen(command,env=env,stdout=log,stderr=log,start_new_session=True)
 spec=importlib.util.spec_from_file_location('corpus',root/'scripts/input-corpus.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 def connect():
  for _ in range(300):
   assert new.poll() is None
   try:return c.Peer(1337)
   except ConnectionRefusedError:time.sleep(.01)
  raise TimeoutError('bridge startup')
 a=connect();a.socket.sendall(c.wire.frame(0x82,b'no-delay'));op,first=a.receive();assert op==2 and len(first)==8
 b=connect();b.socket.sendall(c.wire.frame(0x82,b'no-delay'));op,second=b.receive();assert op==2 and len(second)==8
 payload=bytes(range(256))*4
 for target in [second,bytes([255])*8]:
  a.socket.sendall(c.wire.frame(0x82,target+payload));assert b.receive()==(2,first+payload)
 close=b'\x03\xe8';a.socket.sendall(c.wire.frame(0x88,close));assert a.receive()==(8,close);assert a.socket.recv(1)==b'';a.socket.close()
 assert b.receive()==(2,bytes([255])*8+first)
 b.socket.sendall(c.wire.frame(0x88,close));assert b.receive()==(8,close);assert b.socket.recv(1)==b'';b.socket.close()
 for _ in range(300):
  final=metrics()
  if all(final[key]==0 for key in keys):break
  time.sleep(.01)
 assert all(final[key]==0 for key in keys)
 assert sha(f'/proc/{new.pid}/exe')==record['bridge_sha256']
 assert sha(f'/proc/{original["pids"][0]}/exe')==record['native_sha256_unchanged']
 assert Path(f'/proc/{original["pids"][2]}').exists()
 updated=dict(original);updated['previous_pids']=list(original['pids']);updated['pids']=list(original['pids']);updated['pids'][1]=new.pid
 updated['commands']=list(original['commands']);updated['commands'][1]=command;updated['bridge_sha256']=record['bridge_sha256'];updated['bridge_revision']=build['source_revision'];save(updated)
 record.update(passed=True,new_bridge_pid=new.pid,verified=['private 1KiB binary body','room broadcast 1KiB binary body','departure notification','two exact1000 Close/EOF','native resource retirement'],final_resource_gauges={key:final[key] for key in keys})
except Exception as error:
 record['error']=repr(error)
 if new and new.poll() is None:new.terminate();new.wait(timeout=10)
 if stopped(old):
  with open(root/'artifacts/bridge-fused-manual.log','ab') as log:rollback=subprocess.Popen(original['commands'][1],env=env,stdout=log,stderr=log,start_new_session=True)
  original['pids'][1]=rollback.pid;save(original);record['rollback_pid']=rollback.pid
 else:record['original_process_still_running']=True
 raise
finally:(root/'artifacts/bridge-fused-adoption.json').write_text(json.dumps(record,indent=2)+'\n')
# Keep the convenience executable aligned only after successful live acceptance.
target=root/'target/release/war-legacy-bridge';assert sha(target) in {record['old_bridge_sha256'],record['bridge_sha256']}
temporary=target.with_name('.war-legacy-bridge.fused.new');temporary.write_bytes(exe.read_bytes());temporary.chmod(0o755);temporary.replace(target)
assert sha(target)==record['bridge_sha256']
print(json.dumps(record))
