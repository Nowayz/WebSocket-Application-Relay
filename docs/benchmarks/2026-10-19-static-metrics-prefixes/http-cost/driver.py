"""Fixed-work keep-alive HTTP metrics screen; no data-plane traffic."""
import hashlib,http.client,json,os,socket,statistics,subprocess,time
from pathlib import Path
out=Path('artifacts/metrics-http-cost-20261019.json');assert not out.exists()
assert {0,4}.issubset(os.sched_getaffinity(0));os.sched_setaffinity(0,{4})
expected=Path('docs/benchmarks/2026-10-19-static-metrics-prefixes/http-ready.prom').read_bytes()
record={'passed':False,'requests_per_trial':50000,'warmup_requests':32,'server_cpu':0,'client_cpu':4,'runs':[]}
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def cpu(pid):
 fields=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
 return (int(fields[11])+int(fields[12]))/os.sysconf('SC_CLK_TCK')
for label,kind in [('control-1','control'),('candidate-1','candidate'),('candidate-2','candidate'),('control-2','control')]:
 binary=Path(f'artifacts/candidates/metrics-static-20261019-native-{kind}/war-relay').resolve();admin=port()
 command=['taskset','-c','0',str(binary),'--bind',f'127.0.0.1:{port()}','--admin-bind',f'127.0.0.1:{admin}','--workers','1']
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_ADMIN_TOKEN='metrics-static-fixture',TOKIO_WORKER_THREADS='1',RUST_LOG='info')
 row={'label':label,'kind':kind,'command':command,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'completed':0}
 connection=None
 with Path(f'artifacts/metrics-http-cost-20261019-{label}.log').open('x') as log:
  process=subprocess.Popen(command,env=env,stdout=log,stderr=log)
  try:
   deadline=time.monotonic()+3
   while True:
    assert process.poll() is None and time.monotonic()<deadline
    try:
     connection=http.client.HTTPConnection('127.0.0.1',admin,timeout=2);connection.connect();break
    except OSError:
     connection.close();connection=None;time.sleep(.005)
   headers={'Authorization':'Bearer metrics-static-fixture'}
   def request():
    connection.request('GET','/metrics',headers=headers)
    response=connection.getresponse();body=response.read()
    assert response.status==200 and response.getheader('Content-Type')=='text/plain; version=0.0.4; charset=utf-8'
    assert body==expected
   for _ in range(record['warmup_requests']):request()
   samples=[0]*record['requests_per_trial'];before=cpu(process.pid);client_before=time.process_time()
   started=time.perf_counter_ns();deadline=started+8_000_000_000
   for index in range(len(samples)):
    began=time.perf_counter_ns();assert began<deadline,'fixed-request phase exceeded eight-second watchdog'
    request();samples[index]=time.perf_counter_ns()-began;row['completed']=index+1
   ended=time.perf_counter_ns();client_after=time.process_time();after=cpu(process.pid)
   samples.sort()
   rank=lambda p:samples[(len(samples)*p+99)//100-1]/1e6
   row.update(passed=True,elapsed_seconds=(ended-started)/1e9,server_cpu_seconds=after-before,client_cpu_seconds=client_after-client_before,latency_ms={'median':rank(50),'p95':rank(95),'p99':rank(99),'max':samples[-1]/1e6},body_sha256=hashlib.sha256(expected).hexdigest(),response_bytes=len(expected))
  except BaseException as error:
   row.update(passed=False,error=repr(error));raise
  finally:
   if connection:connection.close()
   if process.poll() is None:process.terminate()
   try:row['exit_code']=process.wait(timeout=3)
   except subprocess.TimeoutExpired:process.kill();row['exit_code']=process.wait(timeout=2);row['forced_kill']=True
   row['passed']=row.get('passed',False) and row['exit_code']==0 and not row.get('forced_kill')
   record['runs'].append(row);out.write_text(json.dumps(record,indent=2)+'\n')
 assert row['passed'];print(json.dumps(row),flush=True)
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
