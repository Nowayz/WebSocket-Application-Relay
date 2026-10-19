import gzip,hashlib,json
from pathlib import Path
r=Path(__file__).resolve().parent
meta=json.loads((r/'provenance.json').read_text())
for name,digest in meta['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
assert 'FAILED (failures=42)' in (r/'before.log').read_text()
selected=(r/'tests.log').read_text();assert 'Ran 15 tests' in selected and '\nOK\n' in selected
source=(r/'plot-user-latency-comparison.py').read_text()
assert 'identity = int(raw_identity)' in source and '0 < identity < 2**64 - 1' in source
assert 'identity = int(raw_identity)' not in gzip.decompress((r/'original.py.gz').read_bytes()).decode()
rows=json.loads((r/'equivalence.json').read_text());assert len(rows)==22 and all(row['unchanged'] for row in rows)
assert {row['users'] for row in rows}=={100000,8192,16}
cli=json.loads((r/'cli.json').read_text());assert cli['passed'] and cli['exit_code']==2 and not cli['outputs_created']
assert 'duplicate user ID' in cli['stderr']
print('Fairness user identity verified: 42 failed-before cases, 15 passing tests, 22 unchanged saved-data comparisons and CLI rejection before writes.')
