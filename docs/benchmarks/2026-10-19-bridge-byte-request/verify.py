import gzip,json,hashlib
from pathlib import Path
p=Path(__file__).resolve().parent
def load(name):
 data=(p/name).read_bytes();return json.loads(gzip.decompress(data) if name.endswith('.gz') else data)
for variant,objects,allocated in [('control',2000,72000),('candidate',1000,38000)]:
 for op in [0,5]:
  r=load(f'{variant}-{op}-heap.json.gz');assert r['passed'] and r['exit_code']==0 and r['server_command'][-2]==str(op) and r['server_command'][-3]=='32'
  a=load(f'{variant}-{op}-analysis.json.gz')['probes'][0];assert a['operations']==1000 and a['totals']==dict(live_objects_delta=0,live_bytes_delta=0,allocated_objects=objects,allocated_bytes=allocated)
  for gate in ['before','after']:
   x=r['probes'][0][gate];raw=Path(x['path'])
   if raw.exists():assert hashlib.sha256(raw.read_bytes()).hexdigest()==x['sha256']
rows=load('timing.json');assert len(rows)==16
for op,size in [(0,32),(0,4096),(5,32),(5,255)]:assert [r['variant'] for r in rows if (r['op'],r['payload_bytes'])==(op,size)]==['control','candidate','candidate','control']
assert '110 passed; 0 failed' in (p/'tests.log').read_text()
js=gzip.decompress((p/'js-check.log.gz').read_bytes()).decode();assert 'tests 61' in js and 'pass 61' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
print('Verified four allocation probes, raw hashes when local, timing order, tests and socket cleanup.')
