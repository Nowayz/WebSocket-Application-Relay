import gzip,json,math
from pathlib import Path
p=Path(__file__).resolve().parent
def load(name):
 data=(p/name).read_bytes()
 return json.loads(gzip.decompress(data) if name.endswith('.gz') else data)
s=load('summary.json');assert s['selected'] and s['frame_bytes_before']==s['frame_bytes_after']==48
analyses=[load(f'{v}-analysis.json.gz')['probes'] for v in ['control','candidate']]
for i,name in enumerate(['assign-u32','assign-u64']):
 a,b=(x[i] for x in analyses);assert a['name']==b['name']==name
 for x in [a,b]:assert x['totals']['live_objects_delta']==x['totals']['live_bytes_delta']==0
 assert a['totals']['allocated_objects']-b['totals']['allocated_objects']==(64 if i else 0)
 assert a['totals']['allocated_bytes']-b['totals']['allocated_bytes']==(576 if i else 0)
for row in s['rows']:
 r=load(row['receipt']);assert r['passed']
 if row['mode']=='paced':
  assert r['correctness_passed'] and r['fanout_offered_load_met'] and r['exit_codes']==[0,0,0]
  assert row['private_max_ms']==r['latency']['max_ms']['max'] and row['fanout_max_ms']==r['fanout_exact_ms']['max_ms']['max']
 else:
  assert r['server_exit_code']==r['client_exit_code']==0 and not r['server_forced_kill']
  assert math.isclose(row['cpu_seconds_per_million_deliveries'],r['mixed']['server_cpu_seconds']*1e6/r['mixed']['counts']['deliveries'])
r=load('js-live-check.json');assert r['passed'] and r['exit_codes']==[0,0] and all(v==0 for v in r['final_resource_gauges'].values())
for v in ['control','candidate']:assert 'Frame bytes: 48' in (p/f'{v}-layout.log').read_text()
print('Verified allocation deltas, layouts, eight traffic receipts and JS cleanup.')
