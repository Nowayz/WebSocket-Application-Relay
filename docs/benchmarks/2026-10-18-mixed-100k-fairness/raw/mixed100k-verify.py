#!/usr/bin/env python3
"""Verify archived files, decoded population samples and primary matched trials."""
import csv,hashlib,io,json,lzma
from pathlib import Path
base=Path(__file__).resolve().parent
for name,expected in json.loads((base/'manifest.json').read_text()).items():
 assert hashlib.sha256((base/name).read_bytes()).hexdigest()==expected,name
for name,expected in json.loads((base/'compressed.json').read_text()).items():
 data=lzma.decompress((base/'raw'/name).read_bytes())
 assert hashlib.sha256(data).hexdigest()==expected['decoded_sha256'],name
 assert len(data)==expected['bytes'],name
campaign=json.loads((base/'raw/mixed100k-final-campaign.json').read_text());assert campaign['passed']
for item in campaign['runs']:
 path=base/'raw'/Path(item['result']).name;r=json.loads(path.read_text())
 assert r['passed'] and r['exit_codes']==[0,0,0],path
 assert r['fanout_windows']['offered']==480 and 0 < r['fanout_windows']['completed'] <= 480,path
 sidecar=next(e['details'] for e in r['sidecar_events'] if e['event']=='measured')
 assert sidecar['fanout']['verified']==r['fanout_windows']['completed']*8192*4,path
 data=lzma.decompress(path.with_suffix('.csv.xz').read_bytes())
 assert hashlib.sha256(data).hexdigest()==r['csv_sha256'],path
 rows=list(csv.DictReader(io.StringIO(data.decode())))
 assert len(rows)==100000 and len({v['id'] for v in rows})==100000
 assert all(int(v['samples'])==120 for v in rows)
 for column,percentiles in r['latency'].items():
  values=sorted(float(v[column]) for v in rows)
  for label,p in [('median',50),('p95',95),('p99',99),('max',100)]:
   assert values[(len(values)-1)*p//100]==percentiles[label],(path,column,label)
 assert hashlib.sha256((base/'raw/mixed100k-final-runner.py').read_bytes()).hexdigest()==r['sha256']['runner']
for label in ['after','before','client','index-experiment','output-experiment','quantum256-experiment','quantum1024-unaccounted-experiment']:
 source=base/'source'/label;record=json.loads((source/'build.json').read_text())
 expected=record.get('sha256',record.get('sources',{}))
 for name,value in expected.items():
  path=source/name
  if name not in ['war-relay','inspection']:
   assert path.exists(),(label,name)
   assert hashlib.sha256(path.read_bytes()).hexdigest()==value,(label,name)
print('Verified archive, all decoded CSV hashes, six complete matched trials, per-user census and percentiles, runner and source identities.')

quiet=json.loads((base/'raw/mixed100k-quiet-final.json').read_text());assert quiet['passed']
for row in quiet['runs']:
 assert row['passed'] and row['users']==100000 and row['samples_each']==120
 data=lzma.decompress((base/'raw'/(Path(row['csv']).name+'.xz')).read_bytes())
 assert hashlib.sha256(data).hexdigest()==row['csv_sha256']
 assert len(list(csv.DictReader(io.StringIO(data.decode()))))==100000
corpus=json.loads((base/'raw/mixed100k-final-corpus.json').read_text())
assert corpus['case']['passed']
for name in ['mixed100k-pressure-final.json','mixed100k-pressure-large-gated.json','mixed100k-selected-profile.json']:
 record=json.loads((base/'raw'/name).read_text());assert record['passed'],name
print('Quiet population, corpus, both pressure scopes and selected profile pass records verified.')

root=base.parents[2]
for name,value in json.loads((base/'dependencies.json').read_text()).items():
 assert hashlib.sha256((root/name).read_bytes()).hexdigest()==value,name
print('Prior evidence dependency manifests verified.')

cpp=json.loads((base/'source/cpp-build.json').read_text())
for name,expected in cpp['source_sha256'].items():
 assert hashlib.sha256((base/'source'/name).read_bytes()).hexdigest()==expected,name
after=json.loads((base/'source/after/build.json').read_text())['sha256']['war-relay']
before=json.loads((base/'source/before/build.json').read_text())['binary_sha256']
sidecar=json.loads((base/'source/client/build.json').read_text())['sha256']['inspection']
capacity=json.loads((base/'source/capacity-client/build.json').read_text())['binary_sha256']
for item in campaign['runs']:
 record=json.loads((base/'raw'/Path(item['result']).name).read_text())
 assert record['sha256']['server']=={'before':before,'after':after,'cpp':cpp['binary_sha256']}[item['kind']]
 assert record['sha256']['sidecar']==sidecar and record['sha256']['capacity_client']==capacity
assert quiet['client_sha256']==capacity
assert quiet['server_sha256']=={'rust':before,'rust-owner':after}
print('C++ source identities and every primary/quiet executable-client pin verified.')
