"""Replace the idle local native test service; preserve its arguments/environment."""
import hashlib,json,os,signal,struct,subprocess,sys,time,urllib.request
from pathlib import Path
root=Path(__file__).resolve().parents[1]
metadata=root/'artifacts/reactor-manual-services.json'
state=json.loads(metadata.read_text())
previous=state['pids'][0]
allowed_cpus=sorted(os.sched_getaffinity(previous))
command=[part.decode() for part in Path(f'/proc/{previous}/cmdline').read_bytes().split(b'\0') if part]
assert command==state['commands'][0], 'manual service identity changed'
environment=dict(item.split('=',1) for item in Path(f'/proc/{previous}/environ').read_bytes().decode().split('\0') if '=' in item)
headers={}
if environment.get('WAR_ADMIN_TOKEN'):
    headers['Authorization']='Bearer '+environment['WAR_ADMIN_TOKEN']
def metrics():
    request=urllib.request.Request('http://127.0.0.1:1339/metrics',headers=headers)
    with urllib.request.urlopen(request,timeout=5) as response:
        return {row.split()[0]:int(row.split()[1]) for row in response.read().decode().splitlines() if row.startswith('war_')}
keys=['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes']
assert all(metrics()[key]==0 for key in keys), 'local service has active resources'
selected=root/'artifacts/candidates/observer-quantum/war-relay'
assert hashlib.sha256(selected.read_bytes()).hexdigest()=='f1bf8bf9fd7c7e8f189806dba4bbdc4441e11ca9f7f680d8d14efe4716cb49be'
os.kill(previous,signal.SIGTERM)
for _ in range(100):
    path=Path(f'/proc/{previous}/stat')
    if not path.exists() or path.read_text().rsplit(')',1)[1].split()[0]=='Z': break
    time.sleep(.05)
else: raise RuntimeError('old native process did not exit')
command[0]=str(selected)
if '--pin-workers' not in command: command.append('--pin-workers')
with (root/'artifacts/observer-quantum-manual-native.log').open('ab') as logs:
    server=subprocess.Popen(['taskset','-c',','.join(map(str,allowed_cpus))]+command,cwd=root,env=environment,stdin=subprocess.DEVNULL,stdout=logs,stderr=logs,start_new_session=True)
for _ in range(100):
    if server.poll() is not None: raise RuntimeError('new native service exited')
    try:
        assert all(metrics()[key]==0 for key in keys)
        break
    except OSError: time.sleep(.05)
else: raise RuntimeError('new native service not ready')
state['previous_pids']=state['pids'][:]
state['pids'][0]=server.pid
state['commands'][0]=command
state['native_sha256']=hashlib.sha256(selected.read_bytes()).hexdigest()
state['base_revision']=json.loads((selected.parent/'build.json').read_text())['base_revision']
state['runtime_patch_sha256']=hashlib.sha256((selected.parent/'runtime.patch').read_bytes()).hexdigest()
state['source_revision']=subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()
metadata.write_text(json.dumps(state,indent=2)+'\n')
client_command=[str(root/'artifacts/baselines/capacity-final/capacity'),'--url','ws://127.0.0.1:1338','--users','32','--room-size','16','--client-workers','2','--source-addresses','2','--seconds','1']
client=subprocess.Popen(client_command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
events=[]
try:
    for row in client.stdout:
        event=json.loads(row)
        events.append(event)
        client.stdin.write('continue\n'); client.stdin.flush()
    error=client.stderr.read()
    assert client.wait(timeout=10)==0,error
finally:
    if client.poll() is None: client.kill(); client.wait()
assert [event['event'] for event in events]==['connected','ready','measured','postcheck','done']
sys.path.insert(0,str(root/'scripts'))
from reference import Client,u64
peers=[]
try:
    peers=[Client(1337,b'affinity-live'),Client(1337,b'affinity-live')]
    sender,receiver=peers
    assert sender.id!=receiver.id
    body=bytes(range(256))*16
    sender.send(2,u64(receiver.id)+body)
    assert receiver.receive()==(2,u64(sender.id)+body)
    close=struct.pack('>H',1000)+b'input batch check'
    for peer in peers: peer.send(8,close)
    for peer in peers:
        while True:
            a,b=peer.exact(2)
            assert a&0x80 and not b&0x80
            size=b&127
            if size==126: size=struct.unpack('>H',peer.exact(2))[0]
            elif size==127: size=struct.unpack('>Q',peer.exact(8))[0]
            payload=peer.exact(size)
            if a&15==9: peer.send(10,payload)
            elif a&15==8:
                assert payload==close
                break
        assert peer.socket.recv(1)==b'', 'legacy EOF missing'
finally:
    for peer in peers: peer.socket.close()
for _ in range(100):
    final=metrics()
    if all(final[key]==0 for key in keys): break
    time.sleep(.05)
else: raise RuntimeError('manual probes did not retire')
with urllib.request.urlopen('http://127.0.0.1:1339/readyz',timeout=5) as response:
    assert response.status==200
with urllib.request.urlopen('http://127.0.0.1:4180',timeout=5) as response:
    assert response.status==200
output={'passed':True,'native_pid':server.pid,'native_cpu_set':sorted(os.sched_getaffinity(server.pid)),'native_sha256':state['native_sha256'],'native_command':command,'native_client_command':client_command,'native_events':events,'legacy_private_body_bytes':4096,'legacy_close_ack_and_eof':True,'final_metrics':final,'readiness_status':200,'static_test_page_status':200,'manual_profile':'Existing general development arguments/environment preserved; OS TCP buffer policy.'}
(root/'artifacts/observer-quantum-manual-acceptance.json').write_text(json.dumps(output,indent=2)+'\n')
print(json.dumps({'passed':True,'native_pid':server.pid,'legacy_close_ack_and_eof':True,'remaining':{key:final[key] for key in keys}}))
