import sys,json,subprocess,os,re,hashlib
from pathlib import Path
kind=sys.argv[1]
output=Path(f'artifacts/channel-count-20261019-{kind}-heaps.json');assert not output.exists()
binary=Path(f'artifacts/candidates/channel-count-20261019-{kind}/channel_invalid_cost')
rows=[]
for iterations in [0,64]:
 prefix=Path(f'artifacts/channel-count-20261019-{kind}-{iterations}-heap').resolve();assert not list(prefix.parent.glob(prefix.name+'*.heap'))
 env={**os.environ,'LD_PRELOAD':'/usr/lib/libtcmalloc.so','HEAPPROFILE':str(prefix),'HEAP_PROFILE_ALLOCATION_INTERVAL':str(100*1024**3),'HEAP_PROFILE_INUSE_INTERVAL':str(100*1024**3)}
 done=subprocess.run([str(binary.resolve()),str(iterations)],env=env,capture_output=True,text=True,check=True)
 files=list(prefix.parent.glob(prefix.name+'*.heap'));assert len(files)==1
 data=files[0].read_bytes();header=data.splitlines()[0].decode();m=re.match(r'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)',header);assert m
 rows.append({'iterations':iterations,'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'heap_path':str(files[0]),'heap_sha256':hashlib.sha256(data).hexdigest(),'counts':list(map(int,m.groups())),'stdout':json.loads(done.stdout),'stderr':done.stderr})
result={'kind':kind,'runs':rows,'allocated_objects_per_conversion':(rows[1]['counts'][2]-rows[0]['counts'][2])/64,'allocated_bytes_per_conversion':(rows[1]['counts'][3]-rows[0]['counts'][3])/64}
output.write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
