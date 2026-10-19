import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
for variant,owners,rebind in [('control',3,False),('candidate',1,True)]:
 names=[variant+'.json']+[f'repeat-{i}-{v}.json' for i,v in enumerate(['control','candidate','candidate','control']) if v==variant]
 for name in names:
  r=json.loads((p/name).read_text());assert r['exit_code']==0 and r['parent_limit_before']==r['parent_limit_after']
  x=r['observation'];assert x['filled_child_thread_slots']==31 and x['owners_after_return']==owners and x['listener_rebind_after_return']==rebind
  assert x==json.loads(r['output'])
a=json.loads((p/'control-build.json').read_text());b=json.loads((p/'candidate-build.json').read_text())
for r in [a,b]:assert r['passed'] and not r['cargo_artifact']['fresh']
assert [k for k in a['source_sha256'].keys()|b['source_sha256'].keys() if a['source_sha256'].get(k)!=b['source_sha256'].get(k)]==['src/server.rs']
assert '110 passed; 0 failed' in (p/'tests.log').read_text()
js=gzip.decompress((p/'js-check.log.gz').read_bytes()).decode();assert 'tests 61' in js and 'pass 61' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
print('Verified six actual OS-fault receipts, parent-limit isolation, source delta, library tests and client cleanup.')
