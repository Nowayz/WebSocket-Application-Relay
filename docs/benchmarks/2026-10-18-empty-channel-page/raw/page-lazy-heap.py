import os,sys,subprocess,signal,json,re
from pathlib import Path
root=Path.cwd();sys.path.insert(0,str(root/'scripts'));from inspection import dump_heap
results=[]
for label in ['control','direct']:
 prefix=root/f'artifacts/page-lazy-heap-{label}'
 env=os.environ.copy();env.update(LD_PRELOAD='/usr/lib/libtcmalloc.so',HEAPPROFILE=str(prefix),HEAPPROFILESIGNAL=str(signal.SIGUSR2),HEAP_PROFILE_ALLOCATION_INTERVAL='0',HEAP_PROFILE_INUSE_INTERVAL='0',PAGE_HEAP_GATES='1')
 with open(str(prefix)+'.stderr','w') as err:
  p=subprocess.Popen(['taskset','-c','7',str(root/f'artifacts/candidates/page-lazy-{label}/screen'),'relay::listing::allocation_screen::management_page_screen','--exact','--ignored','--nocapture'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=err,text=True)
  output=[];gates=[]
  try:
   for line in p.stdout:
    output.append(line)
    if 'PAGE_READY' in line or 'PAGE_DONE' in line:
     gates.append(dump_heap(p.pid,prefix));p.stdin.write('go\n');p.stdin.flush()
   assert p.wait(timeout=10)==0
   assert len(gates)==2
   def cumulative(g):return list(map(int,re.search(r'\[\s*(\d+):\s*(\d+)\]',g['header']).groups()))
   before,after=map(cumulative,gates)
   results.append(dict(label=label,gates=gates,allocations=after[0]-before[0],allocated_bytes=after[1]-before[1]))
   Path(str(prefix)+'.stdout').write_text(''.join(output))
  finally:
   if p.poll() is None:p.kill();p.wait()
Path('artifacts/page-lazy-heap-results.json').write_text(json.dumps(results,indent=2)+'\n');print(json.dumps(results,indent=2))
