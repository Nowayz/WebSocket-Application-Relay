"""Short disposable bridge cancellation test; no long-running service is touched."""
import argparse,hashlib,importlib.util,json,os,socket,subprocess,threading,time,urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def module(name,file):
 spec=importlib.util.spec_from_file_location(name,ROOT/file);value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
join=module('joinwire','scripts/join-pressure.py');legacy=module('legacywire','scripts/reference.py')
def free():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def run(args):
 ports=set()
 while len(ports)<3:ports.add(free())
 native,admin,bridge=ports
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_ADMIN_TOKEN='bridge-shutdown-fixture',TOKIO_WORKER_THREADS='4')
 commands=[[str(args.server.resolve()),'--bind',f'127.0.0.1:{native}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--messages-per-second','0','--bytes-per-second','0','--ping-interval-secs','0','--idle-timeout-secs','0'],[str(args.bridge.resolve()),'--bind',f'127.0.0.1:{bridge}','--upstream',f'ws://127.0.0.1:{native}','--max-connections',str(2*args.users)]]
 record={'passed':False,'users':args.users,'commands':commands,'server_sha256':hashlib.sha256(args.server.read_bytes()).hexdigest(),'bridge_sha256':hashlib.sha256(args.bridge.read_bytes()).hexdigest(),'scope':'Fresh local processes. Bridge has joined peers and incomplete HTTP peers; SIGTERM cancels both. A separate native witness remains connected. Finite lifecycle test, not latency or capacity evidence.'};children=[];logs=[];clients=[];raw=[];witness=None;worker=None;stop=threading.Event();failures=[]
 record['script_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
 record['helper_sha256']={file:hashlib.sha256((ROOT/file).read_bytes()).hexdigest() for file in ['scripts/join-pressure.py','scripts/reference.py','scripts/input-corpus.py','scripts/fragment-fairness.py']}
 def metrics():
  req=urllib.request.Request(f'http://127.0.0.1:{admin}/metrics',headers={'Authorization':'Bearer bridge-shutdown-fixture'})
  with urllib.request.urlopen(req,timeout=2) as r:return {k:int(v) for k,v in (line.split() for line in r.read().decode().splitlines() if line.startswith('war_'))}
 def gate(name,connections):
  limit=time.monotonic()+3
  while True:
   m=metrics()
   retired = connections != 0 or all(m[k] == 0 for k in ['war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes'])
   if m['war_connections']==m['war_sessions']==connections and retired:break
   assert time.monotonic()<limit,(name,m)
   time.sleep(.01)
  record[name]=m;return m
 try:
  for name,command in zip(['native','bridge'],commands):
   log=args.output.with_name(args.output.stem+'-'+name+'.log').open('w');logs.append(log);children.append(subprocess.Popen(command,env=env,stdout=log,stderr=log))
  for _ in range(300):
   assert all(c.poll() is None for c in children)
   try:metrics();s=socket.create_connection(('127.0.0.1',bridge),timeout=.1);s.close();break
   except OSError:time.sleep(.01)
  else:raise TimeoutError('startup')
  witness=join.wire.Peer(native);witness.join(b'shutdown-witness')
  def healthy():
   try:
    seq=0
    while not stop.is_set():join.probe(witness,seq);seq+=1
    record['healthy_verified']=seq
   except BaseException as e:failures.append(repr(e))
  worker=threading.Thread(target=healthy);worker.start()
  legacy_native_bytes=0;legacy_welcome_bytes=0
  for i in range(args.users):
   client=legacy.Client(bridge,f'sd{i:x}'.encode());clients.append(client);body=client.id.to_bytes(8,'little')+bytes(range(64));client.send(2,body);assert client.receive()==(2,body)
   width=1 if client.id<24 else 2 if client.id<256 else 3 if client.id<65536 else 5 if client.id<2**32 else 9
   legacy_welcome_bytes+=width;legacy_native_bytes+=1+width+66
  for _ in range(args.users):
   stream=socket.create_connection(('127.0.0.1',bridge),timeout=2);stream.sendall(b'GET /');raw.append(stream)
  # The acceptor can lag TCP establishment; retry until one excess peer is refused.
  limit=time.monotonic()+2
  while True:
   with socket.create_connection(('127.0.0.1',bridge),timeout=2) as refused:
    refused.settimeout(.1)
    try:
     assert refused.recv(1)==b'';break
    except socket.timeout:pass
   assert time.monotonic()<limit
  before=gate('before_shutdown',args.users+1);assert before['war_channels']==args.users+2
  record['witness_alive_before']=worker.is_alive() and not failures;assert record['witness_alive_before']
  start=time.monotonic();children[1].terminate();record['bridge_exit_code']=children[1].wait(timeout=3);record['bridge_exit_seconds']=time.monotonic()-start;assert record['bridge_exit_code']==0
  for stream in [c.socket for c in clients]+raw:
   stream.settimeout(2)
   try:assert stream.recv(1)==b'','unexpected application bytes during cancellation'
   except ConnectionResetError:pass
   stream.close()
  record['terminated_legacy_transports']=len(clients);record['terminated_partial_http_transports']=len(raw);clients.clear();raw.clear()
  after=gate('after_shutdown',1);assert after['war_channels']==2
  assert after['war_transport_disconnects_total']==args.users
  assert after['war_disconnected_total']==args.users
  assert worker.is_alive() and not failures
  stop.set();worker.join(timeout=5);assert not worker.is_alive() and not failures,failures
  join.probe(witness,2**63);record['post_shutdown_echo_verified']=True
  healthy_count=record['healthy_verified']+1
  delivered_bytes=witness.received_data_bytes+legacy_welcome_bytes+legacy_native_bytes
  join.normal_close(witness);witness=None
  final=gate('final',0)
  expected={'war_accepted_total':args.users+1,'war_joined_total':args.users+1,'war_disconnected_total':args.users+1,'war_transport_disconnects_total':args.users,'war_incoming_messages_total':healthy_count+args.users,'war_incoming_bytes_total':healthy_count*76+legacy_native_bytes,'war_deliveries_total':healthy_count+2*args.users+1,'war_delivered_bytes_total':delivered_bytes,'war_routing_reused_bytes_total':(healthy_count+args.users)*66,'war_channels':1}
  for k,v in expected.items():assert final[k]==v,(k,final[k],v)
  for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total','war_close_failures_total','war_write_errors_total','war_write_timeouts_total','war_protocol_errors_total','war_handshake_errors_total','war_rejected_total','war_slow_consumers_total','war_global_pressure_total']:assert final[k]==0,(k,final[k])
  record.update(passed=True,exact_checks=expected)
 except Exception as error:record['error']=repr(error);raise
 finally:
  stop.set()
  for client in clients:client.socket.close()
  for stream in raw:stream.close()
  if witness:witness.socket.close()
  if worker:worker.join(timeout=6)
  for child in reversed(children):
   if child.poll() is None:child.terminate()
  record['exit_codes']=[]
  for child in children:
   try:code=child.wait(timeout=15)
   except subprocess.TimeoutExpired:
    child.kill();code=child.wait(timeout=5);record.setdefault('forced_kill_pids',[]).append(child.pid)
   record['exit_codes'].append(code)
  record['passed']=record['passed'] and all(code==0 for code in record['exit_codes']) and not record.get('forced_kill_pids')
  for log in logs:log.close()
  args.output.write_text(json.dumps(record,indent=2)+'\n')
 return record
if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--server',type=Path,required=True);p.add_argument('--bridge',type=Path,required=True);p.add_argument('--users',type=int,default=32);p.add_argument('--output',type=Path,required=True);args=p.parse_args();assert args.users>0 and not args.output.exists();args.output.parent.mkdir(parents=True,exist_ok=True);r=run(args);print(json.dumps({k:r.get(k) for k in ['passed','error','bridge_exit_seconds','healthy_verified']}));assert r['passed']
