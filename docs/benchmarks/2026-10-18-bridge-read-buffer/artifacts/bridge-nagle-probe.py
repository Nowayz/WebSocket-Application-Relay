#!/usr/bin/env python3
"""Short Linux delayed-ACK forwarding probe; only disposable local sockets."""
import argparse,base64,hashlib,importlib.util,json,os,queue,socket,struct,subprocess,threading,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('wire',ROOT/'scripts/reference.py');wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
def exact(s,n):
 data=bytearray()
 while len(data)<n:
  part=s.recv(n-len(data));assert part,'unexpected EOF';data.extend(part)
 return bytes(data)
def receive(s):
 a,b=exact(s,2);assert a&0x80 and not a&0x70 and b&0x80
 n=b&127
 if n==126:n=int.from_bytes(exact(s,2),'big')
 elif n==127:n=int.from_bytes(exact(s,8),'big')
 assert n<=64,'fixture packet bound'
 mask=exact(s,4);body=exact(s,n)
 return a&15,bytes(v^mask[i%4] for i,v in enumerate(body))
def frame(op,data):
 assert len(data)<126
 return bytes([0x80|op,len(data)])+data

def run(binary,output,trials):
 record={'passed':False,'bridge':str(binary.resolve()),'bridge_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'scope':'Controlled local delayed ACK, not native relay throughput. Client and fixture server TCP_NODELAY true. Upstream fixture disables QUICKACK before each pair and after first frame, withholds application response until both frames arrive. Parent sends second legacy frame1ms after observed first arrival. Full literal CBOR/legacy data and Close/EOF checked.','trials':[]}
 armed=queue.Queue();first=queue.Queue();results=queue.Queue();failures=[]
 with socket.socket() as listener:
  listener.bind(('127.0.0.1',0));listener.listen();listener.settimeout(3);upstream_port=listener.getsockname()[1]
  with socket.socket() as spare:spare.bind(('127.0.0.1',0));port=spare.getsockname()[1]
  def peer():
   try:
    s,_=listener.accept()
    with s:
     s.settimeout(3);s.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
     header=bytearray()
     while not header.endswith(b'\r\n\r\n'):
      header.extend(exact(s,1));assert len(header)<16384
     fields=dict(line.split(b':',1) for line in bytes(header).split(b'\r\n')[1:] if b':' in line)
     key=fields[b'Sec-WebSocket-Key'].strip()
     accept=base64.b64encode(hashlib.sha1(key+b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
     s.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+b'\r\n\r\n')
     assert receive(s)==(2,b'\x45nagle');s.sendall(frame(2,b'\x18\x2a'))
     for i in range(trials):
      s.setsockopt(socket.IPPROTO_TCP,socket.TCP_QUICKACK,0);armed.put(i)
      op,a=receive(s);ta=time.monotonic_ns();assert (op,a)==(2,b'\x82\x18\x2b\x42a'+bytes([i]))
      s.setsockopt(socket.IPPROTO_TCP,socket.TCP_QUICKACK,0);first.put((i,ta))
      op,b=receive(s);tb=time.monotonic_ns();assert (op,b)==(2,b'\x82\x18\x2b\x42b'+bytes([i]))
      results.put((i,ta,tb));s.sendall(frame(2,a)+frame(2,b))
     assert receive(s)==(8,b'\x03\xe8');s.sendall(frame(8,b'\x03\xe8'))
   except BaseException as error:failures.append(repr(error))
  thread=threading.Thread(target=peer);thread.start()
  env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
  command=[str(binary.resolve()),'--bind',f'127.0.0.1:{port}','--upstream',f'ws://127.0.0.1:{upstream_port}'];record['command']=command
  with output.with_suffix('.bridge.log').open('w') as log:
   process=subprocess.Popen(command,env=env,stdout=log,stderr=log)
   client=None
   try:
    # Readiness checks never consume the one fixture upstream connection.
    for _ in range(300):
     assert process.poll() is None
     try:
      probe=socket.create_connection(('127.0.0.1',port),timeout=.05);probe.close();break
     except OSError:time.sleep(.01)
    client=wire.Client(port,b'nagle');assert client.id==42
    client.socket.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
    for i in range(trials):
     assert armed.get(timeout=3)==i
     sent_first=time.monotonic_ns();client.send(2,struct.pack('<Q',43)+b'a'+bytes([i]))
     j,arrived_first=first.get(timeout=3);assert j==i
     time.sleep(.001)
     sent_second=time.monotonic_ns();client.send(2,struct.pack('<Q',43)+b'b'+bytes([i]))
     j,ta,tb=results.get(timeout=3);assert j==i
     assert client.receive()==(2,struct.pack('<Q',43)+b'a'+bytes([i]))
     assert client.receive()==(2,struct.pack('<Q',43)+b'b'+bytes([i]))
     record['trials'].append({'sequence':i,'first_forward_ms':(ta-sent_first)/1e6,'second_forward_ms':(tb-sent_second)/1e6,'arrival_gap_ms':(tb-ta)/1e6})
    client.send(8,b'\x03\xe8');assert exact(client.socket,4)==b'\x88\x02\x03\xe8';assert client.socket.recv(1)==b''
    thread.join(3);assert not thread.is_alive() and not failures,failures
    record['passed']=True
   finally:
    if client is not None:client.socket.close()
    if process.poll() is None:process.terminate()
    record['exit_code']=process.wait(timeout=5)
    thread.join(4);record['fixture_errors']=failures
    record['passed']=record['passed'] and record['exit_code']==0 and not thread.is_alive()
    output.write_text(json.dumps(record,indent=2)+'\n')
 return record
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--bridge',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--trials',type=int,default=8)
 args=parser.parse_args();assert not args.output.exists() and 1<=args.trials<=255
 x=run(args.bridge,args.output,args.trials);print(json.dumps(x,indent=2));assert x['passed']
