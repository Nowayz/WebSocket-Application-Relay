#!/usr/bin/env python3
import lzma,hashlib,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def read(p):return json.loads(p.read_text())
m=read(ROOT/'manifest.json')
for name,expected in m['files'].items():assert sha(ROOT/name)==expected,name
for name,expected in read(ROOT/'compressed-content.json').items():assert hashlib.sha256(lzma.decompress((ROOT/name).read_bytes())).hexdigest()==expected,name
dep=read(ROOT/'dependency.json');base=(ROOT/dep['path']).resolve();assert sha(base/'manifest.json')==dep['manifest_sha256']
versions=0
for version in (ROOT/'source').iterdir():
 if not (version/'build.json').exists():continue
 b=read(version/'build.json');versions+=1
 for name,expected in b['sources'].items():
  p=version/name
  if not p.exists():p=base/'source/rust'/name
  assert sha(p)==expected,(version.name,name)
for stem,count in [('rust-owner-fanout-comparison',32),('rust-owner-fanout-all-comparison',16),('rust-owner-fanout-isolated-comparison',16),('rust-owner-fanout-explicit-comparison',16)]:
 d=read(ROOT/'raw'/(stem+'.json'));assert d['passed'] and len(d['cases'])==count
 for r in d['cases']:
  trace=ROOT/'raw'/Path(r['trace']).name;assert sha(trace)==r['trace_sha256'];full=read(trace);assert r['passed'] and full['passed'];assert full['server_sha256']==d[r['kind']+'_sha256'];assert full['client_sha256']==d['client_sha256']
for stem in ['all','isolated','explicit']:
 d=read(ROOT/'raw'/('rust-owner-fanout-'+stem+'-capacity.json'));assert d['passed'] and len(d['runs'])==4
 for r in d['runs']:
  assert r['passed'] and r['users']==100000 and r['samples_each']==120
  p=ROOT/'raw'/(Path(r['csv']).name+'.xz');assert hashlib.sha256(lzma.decompress(p.read_bytes())).hexdigest()==r['csv_sha256']
 d=read(ROOT/'raw'/('rust-owner-fanout-'+stem+'-mixed.json'));assert d['passed'] and len(d['runs'])==6 and all(r['passed'] for r in d['runs'])
for name in ['rust-owner-fanout-explicit-corpus','rust-owner-fanout-pressure-v3','rust-owner-fanout-adoption','rust-owner-fanout-explicit-profile']:
 value=read(ROOT/'raw'/(name+'.json'));assert value.get('passed',value.get('case',{}).get('passed')),name
corpus=read(ROOT/'raw/rust-owner-fanout-explicit-corpus.json')
assert corpus['script_sha256']==sha(ROOT/'source/scripts/input-corpus.py')
assert corpus['wire_helper_sha256']==sha(ROOT/'source/scripts/fragment-fairness.py')
print(json.dumps({'passed':True,'hashed_files':len(m['files']),'candidate_source_versions':versions,'final_runtime_commit':read(ROOT/'raw/rust-owner-fanout-adoption.json')['source_revision']}))
