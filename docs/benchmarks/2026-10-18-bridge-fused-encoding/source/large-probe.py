"""Bounded large-frame compatibility probe, disposable sockets only."""
import argparse,base64,hashlib,importlib.util,json,os,socket,struct,subprocess,threading,time
from pathlib import Path
spec=importlib.util.spec_from_file_location('wire',Path('scripts/reference.py'));wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
def exact(s,n):
 b=bytearray()
 while len(b)<n:
  p=s.recv(n-len(b));assert p;b.extend(p)
 return bytes(b)
def recv(s):
 a,b=exact(s,2);n=b&127
 if n==126:n=int.from_bytes(exact(s,2),'big')
 if n==127:n=int.from_bytes(exact(s,8),'big')
 mask=exact(s,4) if b&128 else None;body=exact(s,n)
 return a&15,bytes(x^mask[i%4] for i,x in enumerate(body)) if mask else body
def frame(op,b):
 n=len(b);return bytes([128|op,n]) + b if n<126 else bytes([128|op,126])+struct.pack('>H',n)+b if n<65536 else bytes([128|op,127])+struct.pack('>Q',n)+b
p=argparse.ArgumentParser();p.add_argument('--bridge',type=Path,required=True);p.add_argument('--output',type=Path,required=True);args=p.parse_args();assert not args.output.exists()
r={'passed':False,'sha256':hashlib.sha256(args.bridge.read_bytes()).hexdigest(),'cases':[]};errors=[]
with socket.socket() as listener:
 listener.bind(('127.0.0.1',0));listener.listen();up=listener.getsockname()[1]
 with socket.socket() as spare:spare.bind(('127.0.0.1',0));port=spare.getsockname()[1]
 sizes=[32768,131072,1048576,16*1024*1024-15]
 def peer():
  try:
   s,_=listener.accept()
   with s:
    s.settimeout(10);h=bytearray()
    while not h.endswith(b'\r\n\r\n'):h.extend(exact(s,1))
    key=next(x.split(b':',1)[1].strip() for x in bytes(h).split(b'\r\n') if x.lower().startswith(b'sec-websocket-key:'))
    accept=base64.b64encode(hashlib.sha1(key+b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
    s.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+b'\r\n\r\n');assert recv(s)==(2,b'\x45large');s.sendall(frame(2,b'\x18\x2a'))
    for n in sizes:
     assert recv(s)==(2,b'\x82\x18\x2a\x41x')
     body=bytes(range(256))*(n//256)+bytes(range(n%256));envelope=b'\x82\x1b\x01\x02\x03\x04\x05\x06\x07\x08\x5a'+struct.pack('>I',n)+body
     assert len(envelope)==n+15;s.sendall(frame(2,envelope))
    assert recv(s)==(8,b'\x03\xe8');s.sendall(frame(8,b'\x03\xe8'))
  except BaseException as e:errors.append(repr(e))
 thread=threading.Thread(target=peer,daemon=True);thread.start()
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env['TOKIO_WORKER_THREADS']='4'
 with args.output.with_suffix('.log').open('w') as log:
  process=subprocess.Popen([str(args.bridge.resolve()),'--bind',f'127.0.0.1:{port}','--upstream',f'ws://127.0.0.1:{up}'],env=env,stdout=log,stderr=log)
  try:
   for _ in range(300):
    try:s=socket.create_connection(('127.0.0.1',port),timeout=.1);s.close();break
    except OSError:time.sleep(.01)
   c=wire.Client(port,b'large');assert c.id==42;c.socket.settimeout(10)
   for n in sizes:
    start=time.monotonic();c.send(2,struct.pack('<Q',42)+b'x');op,body=recv(c.socket)
    expected=struct.pack('<Q',0x0102030405060708)+bytes(range(256))*(n//256)+bytes(range(n%256))
    assert op==2 and body==expected;r['cases'].append({'payload_bytes':n,'elapsed_seconds':time.monotonic()-start})
   c.send(8,b'\x03\xe8');assert recv(c.socket)==(8,b'\x03\xe8');assert c.socket.recv(1)==b'';c.socket.close();thread.join(10);assert not thread.is_alive() and not errors,errors;r['passed']=True
  finally:process.terminate();r['exit_code']=process.wait(timeout=10);assert r['exit_code']==0
args.output.write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r))
