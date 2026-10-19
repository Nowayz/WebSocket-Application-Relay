import gzip,hashlib,json
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
assert 'FAILED (failures=6)' in (r/'before.log').read_text()
assert 'Ran 13 tests' in (r/'tests.log').read_text() and '\nOK\n' in (r/'tests.log').read_text()
source=(r/'plot-user-latency-comparison.py').read_text()
assert 'output.exists() or output.is_symlink()' in source
assert source.index('output.exists()')<source.index('import matplotlib')
assert 'output already exists' not in gzip.decompress((r/'original.py.gz').read_bytes()).decode()
x=json.loads((r/'repeat.json').read_text());assert x['passed'] and x['repeat_exit_code']==2 and x['all_outputs_unchanged']
assert len(x['outputs_sha256'])==3 and x['fresh_generation_users']==100000
summary=json.loads((r/'valid-summary.json').read_text());assert summary['statistic']=='max' and summary['cases']['current']['users']==100000
print('Latency output preservation verified: six failed-before collisions, 13 passing tests, valid fresh report and unchanged repeat hashes.')
