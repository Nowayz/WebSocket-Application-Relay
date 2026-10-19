import gzip, hashlib, json
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
before=(r/'before.log').read_text();assert 'Ran 11 tests' in before and 'FAILED (failures=18)' in before
selected=(r/'tests.log').read_text();assert 'Ran 12 tests' in selected and '\nOK\n' in selected
assert 'nearest-rank p99 must select' not in gzip.decompress((r/'original.py.gz').read_bytes()).decode()
assert 'nearest-rank p99 must select' in (r/'plot-user-latency-comparison.py').read_text()
e=json.loads((r/'equivalence.json').read_text());assert len(e)==20 and all(x['numerically_unchanged'] for x in e)
assert {x['users'] for x in e}=={100000,8192}
x=json.loads((r/'current.json').read_text())['cases']['current']
assert x['users']==100000 and x['samples_per_user']=={'minimum':8,'maximum':8}
assert x['p99_sample_scope']['users_whose_p99_rank_selects_maximum']==100000
print('Short p99 validation verified: failed-before cases, 12 passing tests and 20 unchanged saved-data comparisons.')
