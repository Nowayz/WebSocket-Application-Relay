import hashlib,json,os,socket,subprocess,time,urllib.request,urllib.error
from pathlib import Path
output=Path('artifacts/metrics-static-20261019-http.json');assert not output.exists()
record={'passed':False,'runs':[]};expected={}
def port():
 with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
for kind in ['control','candidate']:
 binary=Path(f'artifacts/candidates/metrics-static-20261019-native-{kind}/war-relay').resolve();admin=port()
 command=[str(binary),'--bind',f'127.0.0.1:{port()}','--admin-bind',f'127.0.0.1:{admin}','--workers','1']
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')};env.update(WAR_ADMIN_TOKEN='metrics-static-fixture',TOKIO_WORKER_THREADS='2',RUST_LOG='info')
 row={'kind':kind,'command':command,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'states':[]}
 with Path(f'artifacts/metrics-static-20261019-http-{kind}.log').open('x') as log:
  process=subprocess.Popen(command,env=env,stdout=log,stderr=log)
  def request(path,method='GET'):
   q=urllib.request.Request(f'http://127.0.0.1:{admin}'+path,headers={'Authorization':'Bearer metrics-static-fixture'},method=method,data=b'' if method=='POST' else None)
   try:
    with urllib.request.urlopen(q,timeout=2) as response:return response.status,response.headers,response.read()
   except urllib.error.HTTPError as error:return error.code,error.headers,error.read()
  try:
   deadline=time.monotonic()+3
   while True:
    assert process.poll() is None and time.monotonic()<deadline
    try:status,_,body=request('/metrics')
    except OSError:time.sleep(.01);continue
    assert status==200;break
   for state in ['ready','draining','resumed']:
    if state!='ready':assert request('/v1/drain' if state=='draining' else '/v1/resume','POST')[0]==204
    status,headers,body=request('/metrics');assert status==200 and headers['Content-Type']=='text/plain; version=0.0.4; charset=utf-8'
    values={line.split()[0]:int(line.split()[1]) for line in body.decode().splitlines() if line.startswith('war_')}
    assert len(values)==43 and values['war_draining']==int(state=='draining')
    assert request('/readyz')[0]==(503 if state=='draining' else 200)
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_native_connection_slots','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:assert values[key]==0,key
    if state in expected:assert body==expected[state]
    else:expected[state]=body
    path=Path(f'artifacts/metrics-static-20261019-http-{kind}-{state}.prom');path.write_bytes(body)
    row['states'].append({'state':state,'path':str(path),'sha256':hashlib.sha256(body).hexdigest(),'content_type':headers['Content-Type'],'values':values})
   row['passed']=True
  finally:
   process.terminate()
   try:row['exit_code']=process.wait(timeout=3)
   except subprocess.TimeoutExpired:process.kill();row['exit_code']=process.wait(timeout=2);row['forced_kill']=True
   row['passed']=row.get('passed',False) and row['exit_code']==0 and not row.get('forced_kill')
   record['runs'].append(row);output.write_text(json.dumps(record,indent=2)+'\n')
 assert row['passed'];print(kind,'three exact HTTP states verified')
record['passed']=True;output.write_text(json.dumps(record,indent=2)+'\n')
