import base64,hashlib,json,os,signal,socket,struct,subprocess,time,urllib.request
from pathlib import Path
ROOT=Path.cwd();path=ROOT/'artifacts/reactor-manual-services.json';m=json.loads(path.read_text());old_pid=m['pids'][0]
exe=ROOT/'artifacts/candidates/rust-owner-fanout-explicit/war-relay'
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
assert sha(f'/proc/{old_pid}/exe')==m['native_sha256']
env=dict(v.decode(errors='surrogateescape').split('=',1) for v in Path(f'/proc/{old_pid}/environ').read_bytes().split(b'\0') if b'=' in v)
def metrics():
 req=urllib.request.Request('http://127.0.0.1:1339/metrics',headers={'Authorization':'Bearer '+env.get('WAR_ADMIN_TOKEN','')})
 with urllib.request.urlopen(req,timeout=2) as response:return dict((k,int(v)) for k,v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_')))
before=metrics();assert before['war_connections']==before['war_variable_bytes']==0,'manual service has live state; do not restart'
record={'previous_pid':old_pid,'previous_sha256':m['native_sha256'],'source_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'native_sha256':sha(exe),'passed':False}
command=list(m['commands'][0]);command[0]=str(exe)
os.kill(old_pid,signal.SIGTERM)
for _ in range(500):
 p=Path(f'/proc/{old_pid}/status')
 if not p.exists() or 'State:\tZ' in p.read_text():break
 time.sleep(.02)
with open(ROOT/'artifacts/rust-owner-fanout-manual-server.log','ab') as output:
 server=subprocess.Popen(command,env=env,stdout=output,stderr=output,start_new_session=True)
for _ in range(300):
 assert server.poll() is None,'promoted server exited'
 try:
  with urllib.request.urlopen('http://127.0.0.1:1339/readyz',timeout=.2) as r:assert r.status==200
  break
 except OSError:time.sleep(.02)
else:raise TimeoutError('new manual readiness')
def exact(s,n):
 out=b''
 while len(out)<n:
  piece=s.recv(n-len(out));assert piece,'unexpected EOF';out+=piece
 return out
def receive(s):
 a,b=exact(s,2);assert not b&128;n=b&127
 if n==126:n=struct.unpack('!H',exact(s,2))[0]
 elif n==127:n=struct.unpack('!Q',exact(s,8))[0]
 return a&15,exact(s,n)
def send(s,payload,opcode=2):
 assert len(payload)<126;mask=os.urandom(4);s.sendall(bytes([128|opcode,128|len(payload)])+mask+bytes(x^mask[i%4] for i,x in enumerate(payload)))
def connect():
 s=socket.create_connection(('127.0.0.1',1338),timeout=3);key=base64.b64encode(os.urandom(16)).decode();s.sendall(f'GET / HTTP/1.1\r\nHost: 127.0.0.1:1338\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n'.encode());header=b''
 while not header.endswith(b'\r\n\r\n'):header+=exact(s,1)
 assert header.startswith(b'HTTP/1.1 101'),header
 room=b'fanout-verify';send(s,bytes([0x40+len(room)])+room);opcode,welcome=receive(s);assert opcode==2 and welcome[0]>>5==0
 return s,welcome
first,a=connect();second,b=connect();value=b'\x42ok';broadcast=b'\x1b'+b'\xff'*8
send(first,b'\x82'+b+value);assert receive(second)==(2,b'\x82'+a+value)
send(first,b'\x82'+broadcast+value);assert receive(second)==(2,b'\x82'+a+value)
close=b'\x03\xe8';send(first,close,8);assert receive(first)==(8,close);assert first.recv(1)==b'';first.close()
assert receive(second)==(2,b'\x82'+broadcast+a)
send(second,close,8);assert receive(second)==(8,close);assert second.recv(1)==b'';second.close()
for _ in range(300):
 final=metrics()
 if final['war_connections']==final['war_sessions']==final['war_queued_bytes']==final['war_variable_bytes']==0:break
 time.sleep(.01)
assert final['war_connections']==final['war_sessions']==final['war_queued_bytes']==final['war_variable_bytes']==0
assert sha(f'/proc/{server.pid}/exe')==record['native_sha256']
m['previous_pids']=list(m['pids']);m['pids'][0]=server.pid;m['commands'][0]=command;m['native_sha256']=record['native_sha256'];m['source_revision']=record['source_revision'];m['base_revision']=json.loads((ROOT/'artifacts/candidates/rust-owner-fanout-explicit/build.json').read_text())['base_revision'];m['runtime_patch_sha256']=sha(ROOT/'artifacts/rust-owner-fanout-explicit-runtime.patch');path.write_text(json.dumps(m,indent=2)+'\n')
record.update(passed=True,pid=server.pid,verified=['readiness','executable hash','private delivery','fanout delivery','departure notification','two normal Close/EOF handshakes','resource retirement'],final_resource_gauges={k:final[k] for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes']});(ROOT/'artifacts/rust-owner-fanout-adoption.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({k:v for k,v in record.items() if k!='verified'}))
