"""Verify the preserved bytes and recheck complete population/multicast matrices."""
import csv,hashlib,io,json,lzma,math
from pathlib import Path
root=Path(__file__).resolve().parent
m=json.loads((root/'manifest.json').read_text())
for name,sha in m['files'].items():
 assert hashlib.sha256((root/name).read_bytes()).hexdigest()==sha,name
for name,meta in m['decoded_csv'].items():
 data=lzma.decompress((root/(name+'.xz')).read_bytes())
 assert len(data)==meta['bytes'] and hashlib.sha256(data).hexdigest()==meta['sha256'],name
cases=0;private_probes=0
for path in sorted((root/'raw').glob('*.json')):
 r=json.loads(path.read_text())
 if not (isinstance(r,dict) and 'sidecar_events' in r and 'args' in r and 'csv_sha256' in r):continue
 assert r['passed'] and all(code==0 for code in r['exit_codes']),path.name
 server_name=Path(r['args']['server']).parent.name
 assert hashlib.sha256((root/'sources'/server_name/'war-relay').read_bytes()).hexdigest()==r['sha256']['server']
 sidecar_name=Path(r['args']['sidecar']).parent.name
 if sidecar_name=='examples':sidecar_name='short-fanout-client-retained'
 assert hashlib.sha256((root/'sources'/sidecar_name/'inspection').read_bytes()).hexdigest()==r['sha256']['sidecar']
 assert hashlib.sha256((root/'sources/latency-timeline-client/capacity').read_bytes()).hexdigest()==r['sha256']['capacity_client']
 stem=Path(r['args']['output']).stem
 data=lzma.decompress((root/'raw'/f'{stem}.csv.xz').read_bytes())
 assert hashlib.sha256(data).hexdigest()==r['csv_sha256'],path.name
 rows=list(csv.DictReader(io.StringIO(data.decode())))
 assert len(rows)==r['args']['users'] and len({x['id'] for x in rows})==len(rows),path.name
 samples=r['args']['seconds']*4
 assert all(int(x['samples'])==samples for x in rows),path.name
 private_probes+=len(rows)*samples
 for col in ['p99_ms','max_ms','schedule_p99_ms','schedule_max_ms']:
  values=sorted(float(x[col]) for x in rows)
  assert all(math.isfinite(v) and v>=0 for v in values)
  for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:
   assert values[(len(values)-1)*q//100]==r['latency'][col][label],(path.name,col,label)
 if 'fanout_csv_sha256' in r:
  data=lzma.decompress((root/'raw'/f'{stem}-fanout.csv.xz').read_bytes())
  assert hashlib.sha256(data).hexdigest()==r['fanout_csv_sha256']
  audience=list(csv.DictReader(io.StringIO(data.decode())))
  assert len(audience)==r['args']['readers'] and len({x['id'] for x in audience})==len(audience)
  measured=next(x['details'] for x in r['sidecar_events'] if x['event']=='measured')
  assert sum(int(x['samples']) for x in audience)==measured['fanout']['verified']
  assert all(int(x['samples'])==measured['fanout']['verified']//len(audience) for x in audience)
  for col in ['p99_ns','max_ns','schedule_p99_ns','schedule_max_ns']:
   values=sorted(int(x[col])/1e6 for x in audience)
   for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:
    assert values[(len(values)-1)*q//100]==r['fanout_exact_ms'].get(col, r['fanout_exact_ms'].get(col.removesuffix('_ns') + '_ms'))[label]
 cases+=1
quiet_cases=0
for path in (root/'raw').glob('*.json'):
 r=json.loads(path.read_text())
 if not isinstance(r,dict):continue
 for row in r.get('runs',[]):
  if not isinstance(row,dict) or 'csv_sha256' not in row or 'csv' not in row:continue
  assert row['passed'] and row['client_exit_code']==row['server_exit_code']==0
  data=lzma.decompress((root/'raw'/(Path(row['csv']).name+'.xz')).read_bytes())
  assert hashlib.sha256(data).hexdigest()==row['csv_sha256']
  users=list(csv.DictReader(io.StringIO(data.decode())))
  assert len(users)==row['users'] and len({x['id'] for x in users})==len(users)
  assert all(int(x['samples'])==row['samples_each'] for x in users)
  for col in ['p99_ms','max_ms','schedule_p99_ms','schedule_max_ms']:
   values=sorted(float(x[col]) for x in users)
   for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:assert values[(len(values)-1)*q//100]==row[col][label]
  quiet_cases+=1
print(json.dumps({'quiet_cases':quiet_cases,'verified_files':len(m['files']),'decoded_csv_files':len(m['decoded_csv']),'complete_mixed_cases':cases,'mixed_private_probes':private_probes}))
