import json,os,subprocess,sys,hashlib,re,select,time
from pathlib import Path
sys.path.insert(0,str(Path('scripts').resolve()))
import inspection
out=Path('artifacts/page-borrowed-key-20261019-heaps.json');assert not out.exists();rows=[]
for kind in ['control','candidate']:
 binary=Path(f'artifacts/candidates/page-borrowed-key-20261019-{kind}/page_screen').resolve()
 prefix=Path(f'artifacts/page-borrowed-key-20261019-{kind}-heap').resolve();assert not list(prefix.parent.glob(prefix.name+'*.heap'))
 env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
 env.update(PAGE_HEAP_GATES='1',LD_PRELOAD='/usr/lib/libtcmalloc.so',HEAPPROFILE=str(prefix),HEAPPROFILESIGNAL='12',HEAP_PROFILE_ALLOCATION_INTERVAL=str(100*1024**3),HEAP_PROFILE_INUSE_INTERVAL=str(100*1024**3))
 command=[str(binary),'relay::listing::allocation_screen::management_page_screen','--exact','--ignored','--nocapture']
 process=subprocess.Popen(command,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
 row={'kind':kind,'command':command,'captures':[]}
 try:
  for marker in ['PAGE_READY','PAGE_DONE']:
   deadline=time.monotonic()+10
   while True:
    assert time.monotonic()<deadline and process.poll() is None
    if not select.select([process.stdout],[],[],.05)[0]:continue
    line=process.stdout.readline();assert line
    if marker in line:break
   capture=inspection.dump_heap(process.pid,prefix)
   match=re.match(r'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)',capture['header']);assert match
   capture['counts']=list(map(int,match.groups()));row['captures'].append(capture)
   process.stdin.write('go\n');process.stdin.flush()
  stdout,stderr=process.communicate(timeout=5);assert process.returncode==0
  row.update(passed=True,exit_code=process.returncode)
 finally:
  if process.poll() is None:process.kill();process.wait(timeout=2)
 rows.append(row);out.write_text(json.dumps(rows,indent=2)+'\n');a,b=row['captures'];print(kind,[y-x for x,y in zip(a['counts'],b['counts'])])
