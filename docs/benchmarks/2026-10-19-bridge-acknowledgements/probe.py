import argparse,base64,hashlib,importlib.util,json,os,socket,subprocess,threading,time
from pathlib import Path
spec=importlib.util.spec_from_file_location('wire',Path('scripts/input-corpus.py').resolve());wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
p=argparse.ArgumentParser();p.add_argument('--server',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();assert not args.output.exists()
def exact(s,n):
 b=bytearray()
 while len(b)<n:
  part=s.recv(n-len(b))
  if not part:raise EOFError()
  b.extend(part)
 return bytes(b)
def receive(s):
 first,second=exact(s,2);n=second&127
 if n==126:n=int.from_bytes(exact(s,2),'big')
 if n==127:n=int.from_bytes(exact(s,8),'big')
 assert second&128
 mask=exact(s,4);body=exact(s,n)
 return first&15,bytes(v^mask[i%4] for i,v in enumerate(body))
def send(s,op,body):
 assert len(body)<126;s.sendall(bytes([128|op,len(body)])+body)
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
result={'passed':False,'binary_sha256':hashlib.sha256(args.server.read_bytes()).hexdigest(),'cases':[]}
commands = [
 (0, b'test', b'\x44test', b'\xf5', b'\xf6'),
 (1, b'\x07', b'\x07', b'\x07', b'\x08'),
 (3, (42).to_bytes(8,'little'), b'\x18\x2a', b'\x18\x2a', b'\x00'),
 (4, b'\x01xv', b'\x82\x41x\x41v', b'\xf5', b'\xf4'),
]
cases=[(op,legacy,cbor,good,bad,valid) for op,legacy,cbor,good,bad in commands for valid in [False,True]]
for index,(opcode,legacy_argument,cbor_argument,good,bad,valid) in enumerate(cases):
 errors=[];backend_state=[];front=None;server=None
 with socket.socket() as listener, args.output.with_suffix(f'.{index}.log').open('x') as log:
  listener.bind(('127.0.0.1',0));listener.listen();listener.settimeout(2)
  def upstream():
   try:
    with listener.accept()[0] as peer:
     peer.settimeout(2);header=bytearray()
     while not header.endswith(b'\r\n\r\n'):header.extend(exact(peer,1))
     key=next(line.split(b':',1)[1].strip() for line in header.split(b'\r\n') if line.lower().startswith(b'sec-websocket-key:'))
     accept=base64.b64encode(hashlib.sha1(key+b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
     peer.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+b'\r\n\r\n')
     assert receive(peer)==(2,b'\x4avalidation');send(peer,2,b'\x18\x2a')
     assert receive(peer)==(2,b'\x82\x00\x82\x00\x42ok')
     assert receive(peer)==(2,b'\x82\x00\x82'+bytes([opcode])+cbor_argument)
     assert receive(peer)==(2,b'\x82\x00\x82\x05\x41x')
     auth=b'\x82\x00\x82\x00\xf5'
     ack=b'\x82\x00\x82'+bytes([opcode])+(good if valid else bad)
     missing=b'\x82\x00\x82\x18\xc8\x82\xf4\xf6'
     peer.sendall(b''.join(bytes([0x82,len(body)])+body for body in [auth,ack,missing]))
     try:
      op,body=receive(peer);assert op==8;send(peer,8,body);backend_state.append('close-handshake')
      assert peer.recv(1)==b''
     except EOFError:backend_state.append('eof')
     except ConnectionResetError:backend_state.append('reset')
   except BaseException as e:errors.append(repr(e))
  worker=threading.Thread(target=upstream,daemon=True);worker.start()
  command=[str(args.server.resolve()),'--bind',f'127.0.0.1:{port()}','--upstream',f'ws://127.0.0.1:{listener.getsockname()[1]}','--max-connections','1']
  env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(RUST_LOG='info',TOKIO_WORKER_THREADS='2')
  row={'opcode':opcode,'valid_acknowledgement':valid,'command':command}
  try:
   server=subprocess.Popen(command,env=env,stdout=log,stderr=log)
   end=time.monotonic()+2
   while 'legacy bridge listening' not in args.output.with_suffix(f'.{index}.log').read_text():
    assert server.poll() is None and time.monotonic()<end;time.sleep(.005)
   baseline=len(list(Path(f'/proc/{server.pid}/fd').iterdir()))
   address=command[2];front=wire.Peer(int(address.rsplit(':',1)[1]))
   front.socket.sendall(wire.wire.frame(0x81,b'validation'));assert front.receive()==(2,(42).to_bytes(8,'little'))
   front.socket.sendall(b''.join(wire.wire.frame(0x82,b'\0'*8+packet) for packet in [b'\x00ok',bytes([opcode])+legacy_argument,b'\x05x']));op,body=front.receive()
   row['frontend_opcode']=op;row['frontend_body_hex']=body.hex()
   if op==2:
    assert body==b'\0'*8+b'\xc8'
    front.socket.sendall(wire.wire.frame(0x88,b'\x03\xe8'));assert front.receive()==(8,b'\x03\xe8')
   else:
    assert (op,body)==(8,b'\x03\xebValue Requires CBOR Client')
    front.socket.sendall(wire.wire.frame(0x88,body))
   assert front.socket.recv(1)==b'';front.socket.close();front=None
   worker.join(2);assert not worker.is_alive() and not errors,errors
   end=time.monotonic()+2
   while len(list(Path(f'/proc/{server.pid}/fd').iterdir()))!=baseline:
    assert time.monotonic()<end;time.sleep(.005)
   row.update(fd_baseline=baseline,fd_final=baseline,upstream_retirement=backend_state,passed=True)
  except BaseException as e:row.update(error=repr(e),passed=False);raise
  finally:
   if front:front.socket.close()
   if server:
    if server.poll() is None:server.terminate()
    try:row['exit_code']=server.wait(timeout=3)
    except subprocess.TimeoutExpired:server.kill();row['exit_code']=server.wait(timeout=2);row['forced_kill']=True
    row['passed']=row.get('passed',False) and row['exit_code']==0 and not row.get('forced_kill')
   worker.join(2);row['worker_finished']=not worker.is_alive();result['cases'].append(row);args.output.write_text(json.dumps(result,indent=2)+'\n')
result['passed']=all(r['passed'] and r['worker_finished'] for r in result['cases']);args.output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
