#!/usr/bin/env python3
"""Short real-relay connected memory census with checked small-packet traffic."""
import argparse,concurrent.futures,hashlib,importlib.util,json,os,socket,struct,subprocess,time,urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('wire',ROOT/'scripts/reference.py');wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
def free():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def memory(pid):
 fields={line.split(':',1)[0]:line.split(':',1)[1].split() for line in Path(f'/proc/{pid}/status').read_text().splitlines() if ':' in line}
 stats=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
 return {name:int(fields[name][0]) for name in ['VmRSS','RssAnon','VmHWM','Threads']} | {'cpu_seconds':(int(stats[11])+int(stats[12]))/os.sysconf('SC_CLK_TCK')}
def run(args):
 ports=set()
 while len(ports)<3:ports.add(free())
 native,admin,bridge=ports
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_ADMIN_TOKEN='bridge-buffer-fixture',TOKIO_WORKER_THREADS='4')
 commands=[[str(args.server.resolve()),'--bind',f'127.0.0.1:{native}','--admin-bind',f'127.0.0.1:{admin}','--workers','4','--max-connections',str(args.clients+8),'--max-channels',str(args.clients+1),'--ping-interval-secs','0','--idle-timeout-secs','0'],[str(args.bridge.resolve()),'--bind',f'127.0.0.1:{bridge}','--upstream',f'ws://127.0.0.1:{native}']]
 commands=[['taskset','-c',cpus]+command for cpus,command in zip(['0-3','4-7'],commands)]
 record={'payload_bytes':args.payload_bytes,'passed':False,'commands':commands,'server_sha256':hashlib.sha256(args.server.read_bytes()).hexdigest(),'bridge_sha256':hashlib.sha256(args.bridge.read_bytes()).hexdigest(),'clients':args.clients,'rounds_per_client':args.rounds,'bridge_runtime_workers':4,'scope':'Fresh real-relay private echo processes. Native mask0-3, bridge mask4-7, fixed four workers each; controller mask8-15. CPU gates are quiescent after welcomes and checked complete echo traffic; this is not saturation throughput, 100k capacity, or response latency evidence.'};children=[];logs=[];clients=[]
 def metrics():
  req=urllib.request.Request(f'http://127.0.0.1:{admin}/metrics',headers={'Authorization':'Bearer bridge-buffer-fixture'})
  with urllib.request.urlopen(req,timeout=2) as r:return {k:int(v) for k,v in (line.split() for line in r.read().decode().splitlines() if line.startswith('war_'))}
 def snapshot(name):
  m=metrics();expected=0 if name in ['baseline','retired'] else args.clients
  assert m['war_connections']==m['war_sessions']==expected
  assert m['war_channels']==expected+1
  assert m['war_queued_bytes']==m['war_variable_bytes']==0
  record[name]={'bridge':memory(children[1].pid),'native':memory(children[0].pid),'metrics':m}
 try:
  for name,command in zip(['native','bridge'],commands):
   log=args.output.with_name(args.output.stem+'-'+name+'.log').open('w');logs.append(log);children.append(subprocess.Popen(command,env=env,stdout=log,stderr=log))
  for _ in range(300):
   assert all(c.poll() is None for c in children)
   try:
    metrics();s=socket.create_connection(('127.0.0.1',bridge),timeout=.1);s.close();break
   except OSError:time.sleep(.01)
  snapshot('baseline');start=time.monotonic()
  for i in range(args.clients):
   client=wire.Client(bridge,f'mem{i:x}'.encode());client.socket.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1);clients.append(client)
  assert len({c.id for c in clients})==args.clients
  record['setup_seconds']=time.monotonic()-start;snapshot('joined')
  def traffic(index):
   client=clients[index];target=struct.pack('<Q',client.id);suffix=bytes([index%251])*(args.payload_bytes-8)
   for sequence in range(args.rounds):
    payload=struct.pack('<Q',sequence)+suffix;body=target+payload
    n=len(body);header=(bytes([0x80|n]) if n<126 else b'\xfe'+struct.pack('>H',n) if n<65536 else b'\xff'+struct.pack('>Q',n))
    client.socket.sendall(b'\x82'+header+bytes(4)+body)
    assert client.receive()==(2,body),'sender/body/FIFO mismatch'
   return args.rounds
  began=time.monotonic()
  with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:verified=sum(executor.map(traffic,range(args.clients)))
  assert verified==args.clients*args.rounds
  record.update(verified_probes=verified,traffic_elapsed_seconds=time.monotonic()-began);snapshot('warmed')
  for client in clients:
   client.send(8,b'\x03\xe8');assert client.exact(4)==b'\x88\x02\x03\xe8';assert client.socket.recv(1)==b'';client.socket.close()
  clients.clear()
  for _ in range(300):
   m=metrics()
   if m['war_connections']==m['war_sessions']==0:break
   time.sleep(.01)
  snapshot('retired')
  m=record['retired']['metrics'];assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==args.clients
  assert m['war_incoming_messages_total']==args.clients*args.rounds
  assert m['war_deliveries_total']==args.clients*(args.rounds+1)
  assert m['war_routing_copied_bytes_total']==0
  assert all(v==0 for k,v in m.items() if any(word in k for word in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','aborted_writes_total','close_failures_total','transport_disconnects_total','global_pressure_total']))
  record['passed']=True
 finally:
  for client in clients:client.socket.close()
  for child in reversed(children):
   if child.poll() is None:child.terminate()
  record['exit_codes']=[child.wait(timeout=10) for child in children]
  record['passed']=record['passed'] and all(code==0 for code in record['exit_codes'])
  for log in logs:log.close()
  args.output.write_text(json.dumps(record,indent=2)+'\n')
 return record
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--server',type=Path,required=True);parser.add_argument('--bridge',type=Path,required=True);parser.add_argument('--output',type=Path,required=True);parser.add_argument('--clients',type=int,default=256);parser.add_argument('--rounds',type=int,default=144);parser.add_argument('--payload-bytes',type=int,default=1024);args=parser.parse_args();assert not args.output.exists();r=run(args);print(json.dumps(r,indent=2));assert r['passed']
