import argparse,hashlib,importlib.util,json,os,re,selectors,signal,socket,subprocess,time,urllib.request
from pathlib import Path
spec=importlib.util.spec_from_file_location('join_pressure',Path('scripts/join-pressure.py').resolve());wire=importlib.util.module_from_spec(spec);spec.loader.exec_module(wire)
p=argparse.ArgumentParser();p.add_argument('--server',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--users',type=int,default=8192);p.add_argument('--source-prefix',default='127.210.0');args=p.parse_args()
assert not args.output.exists() and not args.output.with_suffix('.log').exists()
port,admin=wire.free_port(),wire.free_port()
while admin==port: admin=wire.free_port()
command=['taskset','-c','0-7',str(args.server.resolve()),'--bind',f'127.0.0.1:{port}','--admin-bind',f'127.0.0.1:{admin}','--workers','8','--pin-workers','--max-connections',str(args.users+1),'--handshake-timeout-secs','4','--listen-backlog','65535','--ping-interval-secs','0','--idle-timeout-secs','0','--messages-per-second','0','--bytes-per-second','0','--total-read-buffer-bytes','4294967296']
env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env['WAR_ADMIN_TOKEN']='expiry-burst-fixture'
os.sched_setaffinity(0,set(range(8,16)))
record={'passed':False,'diagnostic_only':False,'users':args.users,'source_prefix':args.source_prefix,'command':command,'binary_sha256':hashlib.sha256(args.server.read_bytes()).hexdigest(),'controller_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'events':[]}
def metrics():
 request=urllib.request.Request(f'http://127.0.0.1:{admin}/metrics',headers={'Authorization':'Bearer expiry-burst-fixture'})
 with urllib.request.urlopen(request,timeout=2) as r:return {k:int(v) for k,v in (s.split() for s in r.read().decode().splitlines() if s.startswith('war_'))}
def gate(count):
 limit=time.monotonic()+10
 while True:
  m=metrics()
  if m['war_connections']==count and (count != 0 or all(m[k]==0 for k in ['war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_native_connection_slots'])):return m
  assert time.monotonic()<limit,m
  time.sleep(.01)
server=None;stopped=False;peers=[];witness=None;poller=selectors.DefaultSelector()
with args.output.with_suffix('.log').open('x') as log:
 try:
  server=subprocess.Popen(command,env=env,stdout=log,stderr=log)
  for _ in range(300):
   assert server.poll() is None
   try: metrics();break
   except OSError:time.sleep(.01)
  else:raise TimeoutError('startup')
  record['baseline']=metrics()
  record['baseline_fd_count']=len(list(Path(f'/proc/{server.pid}/fd').iterdir()))
  witness=wire.wire.Peer(port);witness.join(b'expiry-witness');witness.socket.settimeout(20);wire.probe(witness,0)
  ramp=time.monotonic()
  for i in range(args.users):
   if i % 256 == 0: assert time.monotonic()-ramp < 3, "setup exceeded 3-second burst-fixture deadline"
   peer=socket.socket();peers.append(peer);peer.settimeout(5);peer.bind((f'{args.source_prefix}.{2+i%64}',0));peer.connect(('127.0.0.1',port));poller.register(peer,selectors.EVENT_READ)
  pending=gate(args.users+1);record['ramp_seconds']=time.monotonic()-ramp
  assert pending['war_handshake_errors_total']==0 and pending['war_rejected_total']==0,pending
  record['pending']=pending
  os.kill(server.pid,signal.SIGSTOP);stopped=True;pause=time.monotonic();time.sleep(4.05)
  os.kill(server.pid,signal.SIGCONT);stopped=False;record['suspend_seconds']=time.monotonic()-pause
  began=time.monotonic_ns();wire.probe(witness,1);record['first_post_resume_probe_ms']=(time.monotonic_ns()-began)/1e6
  remaining=set(peers);limit=time.monotonic()+15
  while remaining:
   assert time.monotonic()<limit,'expiry EOF deadline'
   for key,_ in poller.select(.1):
    peer=key.fileobj
    assert peer.recv(1)==b'','unexpected raw HTTP data'
    poller.unregister(peer);remaining.remove(peer);peer.close()
  peers.clear();wire.probe(witness,2);record['retired']=gate(1)
  assert record['retired']['war_handshake_errors_total']==args.users
  wire.normal_close(witness);witness=None
  record['final']=gate(0)
  for key in ['war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_native_connection_slots','war_rejected_total','war_routing_copied_bytes_total','war_protocol_errors_total','war_close_failures_total']:
   assert record['final'][key]==0,(key,record['final'][key])
  assert record['final']['war_accepted_total']==args.users+1
  assert record['final']['war_joined_total']==record['final']['war_disconnected_total']==1
  assert record['final']['war_incoming_messages_total']==3
  record['final_fd_count']=len(list(Path(f'/proc/{server.pid}/fd').iterdir()))
  assert record['baseline_fd_count']==record['final_fd_count']
  record['passed']=True
 except BaseException as e: record['error']=repr(e);raise
 finally:
  poller.close()
  for peer in peers:peer.close()
  if witness:witness.socket.close()
  if server:
   if stopped:os.kill(server.pid,signal.SIGCONT)
   if server.poll() is None:server.terminate()
   try:record['server_exit_code']=server.wait(timeout=15)
   except subprocess.TimeoutExpired:server.kill();record['server_exit_code']=server.wait(timeout=5);record['forced_kill']=True
   record['passed']=record['passed'] and record['server_exit_code']==0 and not record.get('forced_kill')
  record['server_log']=args.output.with_suffix('.log').read_text()
  record['timer_bursts']=[list(map(int,m)) for m in re.findall(r'expiry-burst: (\d+) (\d+) (\d+) (\d+)',record['server_log'])]
  args.output.write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({k:record[k] for k in ['passed','ramp_seconds','first_post_resume_probe_ms','timer_bursts']}))
