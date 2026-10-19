"""One-message requested-heap diagnostic at a held Close handshake."""
import argparse,base64,hashlib,importlib.util,json,os,re,signal,socket,subprocess,sys,threading,time
from pathlib import Path
sys.path.insert(0,str(Path('scripts').resolve()))
import inspection
spec=importlib.util.spec_from_file_location('wire',Path('scripts/input-corpus.py').resolve());wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
p=argparse.ArgumentParser();p.add_argument('--bridge',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--kind',choices=['binary','text'],required=True);p.add_argument('--fragmented',action='store_true');a=p.parse_args()
assert not os.path.lexists(a.output)
prefix=a.output.with_suffix('.heap-profile').resolve();assert not list(prefix.parent.glob(prefix.name+'*.heap'))
result={'passed':False,'binary_sha256':hashlib.sha256(a.bridge.read_bytes()).hexdigest(),'diagnostic_only':True,'payload_bytes':4*1024*1024,'kind':a.kind,'fragmented':a.fragmented,'gates':[]}
def exact(s,n):
 out=bytearray()
 while len(out)<n:
  data=s.recv(n-len(out))
  if not data:raise EOFError()
  out.extend(data)
 return bytes(out)
def receive(s):
 first,second=exact(s,2);n=second&127
 if n==126:n=int.from_bytes(exact(s,2),'big')
 if n==127:n=int.from_bytes(exact(s,8),'big')
 assert second&128;mask=exact(s,4);body=exact(s,n)
 return first&15,bytes(v^mask[i%4] for i,v in enumerate(body))
ready=threading.Event();fault=threading.Event();errors=[];worker=None;front=None;server=None
with socket.socket() as listener, socket.socket() as reservation, a.output.with_suffix('.log').open('x') as log:
 listener.bind(('127.0.0.1',0));listener.listen();listener.settimeout(8)
 reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1];reservation.close()
 def upstream():
  try:
   with listener.accept()[0] as peer:
    peer.settimeout(8);header=bytearray()
    while not header.endswith(b'\r\n\r\n'):header.extend(exact(peer,1))
    key=next(line.split(b':',1)[1].strip() for line in header.split(b'\r\n') if line.lower().startswith(b'sec-websocket-key:'))
    accept=base64.b64encode(hashlib.sha1(key+b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
    peer.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+b'\r\n\r\n')
    assert receive(peer)==(2,b'\x49retention')
    peer.sendall(b'\x82\x02\x18\x2a');ready.set();assert fault.wait(8)
    payload=b'Z'*result['payload_bytes']
    body=(b'\x82\x18\x2a\xa1\x00\x5a'+len(payload).to_bytes(4,'big')+payload) if a.kind=='binary' else payload
    result['input_message_bytes']=len(body)
    def send(flags,data):
     n=len(data);length=bytes([n]) if n<126 else b'\x7e'+n.to_bytes(2,'big') if n<=65535 else b'\x7f'+n.to_bytes(8,'big')
     peer.sendall(bytes([flags])+length+data)
    opcode=2 if a.kind=='binary' else 1
    if a.fragmented:
     middle=len(body)//2;send(opcode,body[:middle]);send(0x80,body[middle:])
    else:send(0x80|opcode,body)
    assert peer.recv(1)==b''
  except BaseException as error:errors.append(repr(error));ready.set()
 worker=threading.Thread(target=upstream,daemon=True);worker.start()
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
 env.update(TOKIO_WORKER_THREADS='2',RUST_LOG='info',LD_PRELOAD='/usr/lib/libtcmalloc.so',HEAPPROFILE=str(prefix),HEAPPROFILESIGNAL=str(signal.SIGUSR2),HEAP_PROFILE_ALLOCATION_INTERVAL=str(100*1024**3),HEAP_PROFILE_INUSE_INTERVAL=str(100*1024**3))
 command=[str(a.bridge.resolve()),'--bind',f'127.0.0.1:{port}','--upstream',f'ws://127.0.0.1:{listener.getsockname()[1]}','--max-connections','1'];result['command']=command
 def capture(phase):
  row=inspection.dump_heap(server.pid,prefix);row['phase']=phase
  counts=re.match(r'heap profile:\s*(\d+):\s*(\d+)',row['header']);assert counts;row['live_objects']=int(counts[1]);row['live_bytes']=int(counts[2]);result['gates'].append(row)
 try:
  server=subprocess.Popen(command,env=env,stdout=log,stderr=log)
  deadline=time.monotonic()+3
  while 'legacy bridge listening' not in a.output.with_suffix('.log').read_text():
   assert server.poll() is None and time.monotonic()<deadline;time.sleep(.005)
  baseline_fds=len(list(Path(f'/proc/{server.pid}/fd').iterdir()))
  front=wire.Peer(port);front.socket.settimeout(3)
  front.socket.sendall(wire.wire.frame(0x82,b'retention'));assert ready.wait(3) and not errors,errors
  assert front.receive()==(2,(42).to_bytes(8,'little'))
  capture('joined')
  fault.set()
  expected=b'\x03\xebValue Requires CBOR Client' if a.kind=='binary' else b'\x03\xeaInvalid Upstream Message'
  received=front.receive();assert received==(8,expected),received
  result['frontend_close_body_hex']=received[1].hex()
  capture('closing_without_frontend_ack')
  front.socket.sendall(wire.wire.frame(0x88,expected))
  assert front.socket.recv(1)==b'';front.socket.close();front=None
  worker.join(3);assert not worker.is_alive() and not errors,errors
  deadline=time.monotonic()+3
  while len(list(Path(f'/proc/{server.pid}/fd').iterdir()))!=baseline_fds:
   assert time.monotonic()<deadline;time.sleep(.005)
  capture('retired');result.update(passed=True,fd_baseline=baseline_fds,fd_final=baseline_fds)
 finally:
  fault.set()
  if front:front.socket.close()
  if server:
   if server.poll() is None:server.terminate()
   try:result['exit_code']=server.wait(timeout=3)
   except subprocess.TimeoutExpired:server.kill();result['exit_code']=server.wait(timeout=2);result['forced_kill']=True
  worker.join(3);result['worker_finished']=not worker.is_alive();result['upstream_errors']=errors
  result['passed']=result['passed'] and result.get('exit_code')==0 and result['worker_finished'] and not errors
  a.output.write_text(json.dumps(result,indent=2)+'\n')
assert result['passed'];print(json.dumps({'passed':True,'gates':[(g['phase'],g['live_bytes']) for g in result['gates']]}))
