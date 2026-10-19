import gzip,json,hashlib
from pathlib import Path
p=Path(__file__).resolve().parent
def load(name):
 data=(p/name).read_bytes();return json.loads(gzip.decompress(data) if name.endswith('.gz') else data)
for variant,objects,allocated in [('control',2000,65571000),('candidate',1000,32780000)]:
 r=load(variant+'-heap.json.gz');assert r['passed'] and r['exit_code']==0
 a=load(variant+'-analysis.json.gz');probe=a['probes'][0]
 assert probe['operations']==1000 and probe['totals']==dict(live_objects_delta=0,live_bytes_delta=0,allocated_objects=objects,allocated_bytes=allocated)
 for gate in ['before','after']:
  x=r['probes'][0][gate];raw=Path(x['path'])
  if raw.exists():assert hashlib.sha256(raw.read_bytes()).hexdigest()==x['sha256']
timing=load('timing.json');assert len(timing)==20
for size in [0,24,1024,32768,65536]:assert [r['variant'] for r in timing if r['payload_bytes']==size]==['control','candidate','candidate','control']
r=load('js-live-check.json');assert r['passed'] and r['exit_codes']==[0,0] and all(v==0 for v in r['final_resource_gauges'].values())
assert '108 passed; 0 failed' in (p/'tests.log').read_text()
print('Verified allocation deltas, raw hashes when local, timing shape, tests and socket cleanup.')
