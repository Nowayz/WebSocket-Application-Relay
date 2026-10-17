import subprocess,time,os,pathlib
root=pathlib.Path.cwd();env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
with open('artifacts/cpp-private-allocation-profile-server.log','w') as log:
 server=subprocess.Popen(['taskset','-c','0-3',str(root/'artifacts/candidates/cpp-modern/war-relay-cpp'),'--bind','127.0.0.1:29172','--workers','4','--pin-workers','--no-admin'],env=env,stdout=log,stderr=log)
 try:
  time.sleep(.2)
  perf=subprocess.Popen(['perf','record','-e','cycles:u','-F','199','--call-graph','dwarf,8192','-o','artifacts/cpp-private-allocation.perf.data','-p',str(server.pid),'--','sleep','7'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
  load=subprocess.run(['taskset','-c','8-15',str(root/'artifacts/baselines/capacity-final/stress'),'--url','ws://127.0.0.1:29172','--mode','traffic','--clients','64','--duration-secs','6','--window','16','--payload-bytes','1024'],capture_output=True,text=True,timeout=30)
  pathlib.Path('artifacts/cpp-private-allocation-profile-client.log').write_text(load.stdout+load.stderr)
  out,err=perf.communicate(timeout=15);pathlib.Path('artifacts/cpp-private-allocation-profile-perf.log').write_bytes(out+err)
  assert load.returncode==perf.returncode==0
 finally:
  server.terminate();server.wait(timeout=10)
