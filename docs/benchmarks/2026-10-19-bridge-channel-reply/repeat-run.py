import subprocess,json,os,hashlib,re
from pathlib import Path
out=Path('artifacts/bridge-channel-reply-20261019-final-repeat.json');assert not out.exists()
cpu=min(os.sched_getaffinity(0));os.sched_setaffinity(0,{cpu})
record={'passed':False,'cpu':cpu,'ordinary':[],'allocation_diagnostics':[]}
for channels,iterations in [(0,200000),(1,200000),(64,10000),(1024,1000),(4096,250)]:
 for kind in ['final','control','control','final','final','control']:
  binary=Path(f'artifacts/candidates/bridge-channel-reply-20261019-{kind}/bridge_channel_reply_cost')
  cmd=[str(binary),str(channels),str(iterations)]
  row=json.loads(subprocess.check_output(cmd,text=True));row.update(kind=kind,command=cmd,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest());record['ordinary'].append(row)
for kind in []:
 for iterations in ([0,1000] if kind == 'final' else []):
  binary=Path(f'artifacts/candidates/bridge-channel-reply-20261019-{kind}/bridge_channel_reply_cost')
  prefix=Path(f'artifacts/bridge-channel-reply-20261019-{kind}-{iterations}-heap').resolve();assert not list(prefix.parent.glob(prefix.name+'*'))
  env=os.environ.copy();env.update(LD_PRELOAD='/usr/lib/libtcmalloc.so',HEAPPROFILE=str(prefix),HEAP_PROFILE_ALLOCATION_INTERVAL='107374182400',HEAP_PROFILE_INUSE_INTERVAL='107374182400')
  cmd=[str(binary),'1024',str(iterations)]
  done=subprocess.run(cmd,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,check=True)
  heaps=list(prefix.parent.glob(prefix.name+'*.heap'));assert len(heaps)==1
  header=heaps[0].read_text().splitlines()[0];match=re.search(r'\[\s*(\d+):\s*(\d+)\]',header);assert match
  record['allocation_diagnostics'].append({'kind':kind,'iterations':iterations,'command':cmd,'heap_path':str(heaps[0]),'header':header,'total_objects':int(match[1]),'total_bytes':int(match[2]),'heap_sha256':hashlib.sha256(heaps[0].read_bytes()).hexdigest(),'stdout':done.stdout,'stderr':done.stderr})
record['passed']=True;out.write_text(json.dumps(record,indent=2)+'\n')
