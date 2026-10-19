import os,resource,subprocess,json,sys,hashlib,time
from pathlib import Path
binary=Path(sys.argv[1]).resolve();output=Path(sys.argv[2]);assert not output.exists()
uid=os.getuid();threads=0
for path in Path('/proc').glob('[0-9]*/status'):
 try:
  fields=dict(line.split(':',1) for line in path.read_text().splitlines() if ':' in line)
  if int(fields['Uid'].split()[0])==uid:threads+=int(fields['Threads'])
 except (FileNotFoundError,ProcessLookupError,PermissionError):pass
original=resource.getrlimit(resource.RLIMIT_NPROC);limit=threads+32
if original[1]!=resource.RLIM_INFINITY:limit=min(limit,original[1])
def child_limit():resource.setrlimit(resource.RLIMIT_NPROC,(limit,original[1]))
command=['taskset','-c',str(min(os.sched_getaffinity(0))),str(binary)];began=time.monotonic();result=subprocess.run(command,preexec_fn=child_limit,capture_output=True,text=True,timeout=15)
r={'command':command,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'uid_threads_before':threads,'child_thread_limit':limit,'parent_limit_before':original,'parent_limit_after':resource.getrlimit(resource.RLIMIT_NPROC),'exit_code':result.returncode,'output':result.stdout,'stderr':result.stderr,'elapsed_seconds':time.monotonic()-began}
assert r['parent_limit_before']==r['parent_limit_after'];output.write_text(json.dumps(r,indent=2)+'\n');assert result.returncode==0,result.stderr
r['observation']=json.loads(result.stdout);output.write_text(json.dumps(r,indent=2)+'\n');print(r['observation'])
