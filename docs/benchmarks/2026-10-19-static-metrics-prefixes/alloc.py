import json,subprocess,os,re,hashlib
from pathlib import Path
out=Path('artifacts/metrics-static-20261019-allocations.json');assert not out.exists();rows=[]
for kind in ['control','candidate']:
 binary=Path(f'artifacts/candidates/metrics-static-20261019-{kind}/metrics_static_cost').resolve()
 for case in ['zero','max','budget']:
  captures=[]
  for iterations in [0,64]:
   prefix=Path(f'artifacts/metrics-static-20261019-{kind}-{case}-{iterations}-heap').resolve();assert not list(prefix.parent.glob(prefix.name+'*.heap'))
   env={**os.environ,'LD_PRELOAD':'/usr/lib/libtcmalloc.so','HEAPPROFILE':str(prefix),'HEAP_PROFILE_ALLOCATION_INTERVAL':str(100*1024**3),'HEAP_PROFILE_INUSE_INTERVAL':str(100*1024**3)}
   command=[str(binary),case,'-',str(iterations),'1'];done=subprocess.run(command,env=env,capture_output=True,text=True,check=True)
   files=list(prefix.parent.glob(prefix.name+'*.heap'));assert len(files)==1
   data=files[0].read_bytes();header=data.splitlines()[0].decode();m=re.match(r'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)',header);assert m
   captures.append({'iterations':iterations,'path':str(files[0]),'sha256':hashlib.sha256(data).hexdigest(),'counts':list(map(int,m.groups())),'command':command})
  a,b=captures;row={'kind':kind,'case':case,'captures':captures,'objects_per_render':(b['counts'][2]-a['counts'][2])/64,'bytes_per_render':(b['counts'][3]-a['counts'][3])/64}
  rows.append(row);out.write_text(json.dumps(rows,indent=2)+'\n');print(kind,case,row['objects_per_render'],row['bytes_per_render'])
sizes={}
for kind in ['control','candidate']:
 binary=Path(f'artifacts/candidates/metrics-static-20261019-{kind}/metrics_static_cost')
 result=subprocess.check_output(['size','-A',str(binary)],text=True)
 sizes[kind]={line.split()[0]:int(line.split()[1]) for line in result.splitlines() if len(line.split())==3 and line.split()[0] in ['.text','.rodata']}
Path('artifacts/metrics-static-20261019-sections.json').write_text(json.dumps(sizes,indent=2)+'\n');print(sizes)
