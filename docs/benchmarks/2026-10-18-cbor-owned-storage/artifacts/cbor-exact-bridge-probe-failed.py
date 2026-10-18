"""Promote the tested bridge only when no manual client/state can be interrupted."""
import hashlib,importlib.util,json,os,signal,subprocess,time,urllib.request
from pathlib import Path
root=Path.cwd();path=root/'artifacts/reactor-manual-services.json';m=json.loads(path.read_text());old=m['pids'][1]
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert sha(f'/proc/{old}/exe')==m['bridge_sha256']
assert not subprocess.check_output(['ss','-Htn','state','established','(','sport','=',':1337',')'],text=True).strip(),'bridge has live clients'
native_env=dict(v.decode(errors='surrogateescape').split('=',1) for v in Path(f'/proc/{m["pids"][0]}/environ').read_bytes().split(b'\0') if b'=' in v)
env=dict(v.decode(errors='surrogateescape').split('=',1) for v in Path(f'/proc/{old}/environ').read_bytes().split(b'\0') if b'=' in v)
def metrics():
 req=urllib.request.Request('http://127.0.0.1:1339/metrics',headers={'Authorization':'Bearer '+native_env.get('WAR_ADMIN_TOKEN','')})
 with urllib.request.urlopen(req,timeout=2) as r:return dict((k,int(v)) for k,v in (line.split() for line in r.read().decode().splitlines() if line.startswith('war_')))
keys=['war_connections','war_sessions','war_queued_bytes','war_variable_bytes']
assert all(metrics()[key]==0 for key in keys),'manual native service has live state'
exe=root/'artifacts/candidates/cbor-exact-bridge/war-legacy-bridge';record={'passed':False,'old_bridge_pid':old,'old_bridge_sha256':m['bridge_sha256'],'bridge_sha256':sha(exe),'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'native_pid_unchanged':m['pids'][0],'native_sha256_unchanged':sha(f'/proc/{m["pids"][0]}/exe')}
command=list(m['commands'][1]);command[0]=str(exe)
old_command=list(m['commands'][1]);new=None
try:
 os.kill(old,signal.SIGTERM)
 for _ in range(500):
  status=Path(f'/proc/{old}/status')
  if not status.exists() or 'State:\tZ' in status.read_text():break
  time.sleep(.01)
 with open(root/'artifacts/cbor-exact-manual-bridge.log','ab') as log:new=subprocess.Popen(command,env=env,stdout=log,stderr=log,start_new_session=True)
 spec=importlib.util.spec_from_file_location('corpus',root/'scripts/input-corpus.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 def connect():
  for _ in range(300):
   assert new.poll() is None
   try:return c.Peer(1337)
   except ConnectionRefusedError:time.sleep(.01)
  raise TimeoutError('bridge startup')
 a=connect();a.socket.sendall(c.wire.frame(0x81,b'exact-storage-live'));op,first=a.receive();assert op==2 and len(first)==8
 b=connect();b.socket.sendall(c.wire.frame(0x81,b'exact-storage-live'));op,second=b.receive();assert op==2 and len(second)==8
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
 assert sha(f'/proc/{m["pids"][0]}/exe')==record['native_sha256_unchanged']
 m['previous_pids']=list(m['pids']);m['pids'][1]=new.pid;m['commands'][1]=command;m['bridge_sha256']=record['bridge_sha256'];m['bridge_revision']=record['source_revision'];path.write_text(json.dumps(m,indent=2)+'\n')
 record.update(passed=True,new_bridge_pid=new.pid,verified=['private 1KiB binary body','room broadcast 1KiB binary body','departure notification','two exact1000 Close/EOF','native resource retirement'],final_resource_gauges={key:final[key] for key in keys})
except Exception as error:
 record['error']=repr(error)
 if new and new.poll() is None:new.terminate();new.wait(timeout=10)
 with open(root/'artifacts/cbor-exact-manual-bridge.log','ab') as log:rollback=subprocess.Popen(old_command,env=env,stdout=log,stderr=log,start_new_session=True)
 m['pids'][1]=rollback.pid;path.write_text(json.dumps(m,indent=2)+'\n');record['rollback_pid']=rollback.pid
 raise
finally:(root/'artifacts/cbor-exact-bridge-adoption.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record))
