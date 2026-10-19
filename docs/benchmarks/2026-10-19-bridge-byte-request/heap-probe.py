import sys,json,subprocess,os,signal,hashlib,tempfile
from pathlib import Path
sys.path.insert(0,str(Path.cwd()/'scripts'));import inspection
binary=Path(sys.argv[1]).resolve();prefix=Path(sys.argv[2]).resolve();output=Path(sys.argv[3]);assert not output.exists() and not list(prefix.parent.glob(prefix.name+'*.heap'))
lib=Path('/usr/lib/libtcmalloc.so').resolve();env=dict(os.environ,LD_PRELOAD=str(lib),HEAPPROFILE=str(prefix),HEAPPROFILESIGNAL=str(signal.SIGUSR2),HEAP_PROFILE_ALLOCATION_INTERVAL=str(100*1024**3),HEAP_PROFILE_INUSE_INTERVAL=str(100*1024**3))
cmd=['taskset','-c','4-7',str(binary),'32',sys.argv[4],'heap'];r={'passed':False,'server_command':cmd,'server_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'library':str(lib),'library_sha256':hashlib.sha256(lib.read_bytes()).hexdigest(),'probes':[]}
with tempfile.TemporaryFile() as log:
 p=subprocess.Popen(cmd,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True)
 try:
  p.stdin.write('INIT\n');p.stdin.flush();assert p.stdout.readline().strip()=='READY'
  before=inspection.dump_heap(p.pid,prefix);p.stdin.write('GO\n');p.stdin.flush();assert p.stdout.readline().strip()=='DONE';after=inspection.dump_heap(p.pid,prefix)
  r['probes']=[{'name':'byte-command-32','operations':1000,'before':before,'after':after}]
  p.stdin.write('EXIT\n');p.stdin.flush();assert p.wait(timeout=5)==0;r.update(passed=True,exit_code=p.returncode)
 finally:
  if p.poll() is None:p.kill();p.wait(timeout=5)
  log.seek(0);r['log']=log.read().decode();output.write_text(json.dumps(r,indent=2)+'\n')
print(r['passed'])
