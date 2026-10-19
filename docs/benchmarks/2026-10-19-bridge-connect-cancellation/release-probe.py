"""Release bridge: control frames/EOF cancel an uncompleted upstream upgrade."""
import hashlib,importlib.util,json,os,queue,signal,socket,struct,subprocess,sys,tempfile,threading,time
from pathlib import Path
spec=importlib.util.spec_from_file_location('wire',Path.cwd()/'scripts/reference.py')
wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)

class Client(wire.Client):
 def __init__(self,port):
  self.socket=socket.create_connection(('127.0.0.1',port),timeout=2);self.closed=False
  self.socket.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n')
  header=bytearray()
  while not header.endswith(b'\r\n\r\n'):header.extend(self.exact(1))
  assert header.startswith(b'HTTP/1.1 101 ')
  self.send(2,b'upgrade-cancel')
 def control(self):
  a,b=self.exact(2);assert a&0x80 and not b&0x80 and b<126
  return a&15,self.exact(b)

def main():
 binary=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]);record={'passed':False,'cases':[]}
 with output.open('x') as receipt,socket.socket() as upstream,socket.socket() as reservation,tempfile.TemporaryFile() as log:
  upstream.bind(('127.0.0.1',0));upstream.listen(1);upstream.settimeout(2)
  reservation.bind(('127.0.0.1',0));port=reservation.getsockname()[1];reservation.close()
  command=[str(binary),'--bind',f'127.0.0.1:{port}','--upstream',f'ws://127.0.0.1:{upstream.getsockname()[1]}','--max-connections','1']
  env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(TOKIO_WORKER_THREADS='2',RUST_LOG='info')
  process=subprocess.Popen(command,env=env,stdout=log,stderr=log)
  record.update(command=command,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),helper_sha256=hashlib.sha256(Path('scripts/reference.py').read_bytes()).hexdigest())
  fds=lambda:len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
  worker=None;client=None
  try:
   deadline=time.monotonic()+3
   while b'legacy bridge listening' not in os.pread(log.fileno(),65536,0):
    assert process.poll() is None and time.monotonic()<deadline;time.sleep(.005)
   record['baseline_fds']=fds()
   for abrupt in [False,True]:
    events=queue.Queue()
    def stall():
     try:
      stream,_=upstream.accept()
      with stream:
       stream.settimeout(2);header=bytearray()
       while not header.endswith(b'\r\n\r\n'):
        part=stream.recv(1);assert part;header.extend(part)
       assert header.startswith(b'GET / HTTP/1.1\r\n');events.put('ready')
       assert stream.recv(1)==b''
      events.put('retired')
     except BaseException as error:events.put(repr(error))
    worker=threading.Thread(target=stall,daemon=True);worker.start()
    client=Client(port);assert events.get(timeout=2)=='ready'
    started=time.monotonic()
    if abrupt:client.socket.close();client=None
    else:
     client.send(9,b'waiting');assert client.control()==(10,b'waiting')
     close=struct.pack('>H',1000)+b'cancel upgrade';client.send(8,close)
     assert client.control()==(8,close)
     assert client.socket.recv(1)==b''
     client.socket.close();client=None
    assert events.get(timeout=2)=='retired';worker.join(timeout=2);assert not worker.is_alive()
    elapsed=time.monotonic()-started
    deadline=time.monotonic()+2
    while fds()!=record['baseline_fds']:
     assert time.monotonic()<deadline;time.sleep(.005)
    record['cases'].append({'case':'eof' if abrupt else 'ping-then-close','elapsed_seconds':elapsed,'upstream_eof':True,'fds':fds()})
   record['final_fds']=fds();record['passed']=True
  except BaseException as error:record['error']=repr(error)
  finally:
   if client:client.socket.close()
   if process.poll() is None:process.send_signal(signal.SIGTERM)
   try:record['exit_code']=process.wait(timeout=3)
   except subprocess.TimeoutExpired:
    process.kill();record['exit_code']=process.wait(timeout=2);record['forced_kill']=True
   if worker:worker.join(timeout=3)
   record['passed']=record['passed'] and record['exit_code']==0 and not record.get('forced_kill')
   log.seek(0);record['log']=log.read().decode(errors='replace')
   receipt.write(json.dumps(record,indent=2)+'\n')
 print(json.dumps(record));assert record['passed'],record.get('error')
if __name__=='__main__':main()
