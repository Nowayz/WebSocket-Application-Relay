import gzip,json,hashlib,statistics
from pathlib import Path
p=Path(__file__).resolve().parent
def load(name):
 data=(p/name).read_bytes();return json.loads(gzip.decompress(data) if name.endswith('.gz') else data)
receipts=load('heap-receipts.json.gz');analyses=load('heap-analyses.json.gz')
expected={'control-v2':{1:(3000,38000),3:(2000,22000),0:(1000,38000),5:(1000,38000),4:(1000,32780000)},'direct':{1:(1000,5000),3:(1000,13000),0:(1000,38000),5:(1000,38000),4:(1000,32780000)}}
for variant,cases in expected.items():
 for op,(objects,allocated) in cases.items():
  r=receipts[variant][str(op)];assert r['passed'] and r['exit_code']==0 and r['server_command'][-3]==str(op)
  probe=analyses[variant][str(op)]['probes'][0];assert probe['operations']==1000 and probe['totals']==dict(live_objects_delta=0,live_bytes_delta=0,allocated_objects=objects,allocated_bytes=allocated)
  for gate in ['before','after']:
   desc=r['probes'][0][gate];raw=Path(desc['path'])
   if raw.exists():assert hashlib.sha256(raw.read_bytes()).hexdigest()==desc['sha256']
a,b=load('control-build.json'),load('direct-build.json');assert a['passed'] and b['passed'] and not a['cargo_artifact']['fresh'] and not b['cargo_artifact']['fresh']
assert [k for k in a['source_sha256'].keys()|b['source_sha256'].keys() if a['source_sha256'].get(k)!=b['source_sha256'].get(k)]==['src/legacy_bridge.rs']
rows=load('timing.json');assert len(rows)==20
for op in [1,3,0,5,4]:assert [r['variant'] for r in rows if r['op']==op]==['control-v2','direct','direct','control-v2']
for r in rows:assert len(r['samples_ns'])==7 and statistics.median(r['samples_ns'])==r['ns_per_conversion']
assert '111 passed; 0 failed' in (p/'tests.log').read_text()
js=gzip.decompress((p/'js-check.log.gz').read_bytes()).decode();assert 'tests 61' in js and 'pass 61' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
print('Verified ten allocation probes, matching source scope, pinned sample medians, wire tests and socket cleanup.')
