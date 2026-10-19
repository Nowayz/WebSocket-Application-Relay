import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
before=(p/'before.log').read_text();assert 'capacity overflow' in before and 'panicked' in before
r=json.loads((p/'cli.json').read_text());assert r['exit_code']==1 and 'cannot reserve native worker storage' in r['output'] and 'panicked' not in r['output']
b=json.loads((p/'build.json').read_text());assert b['passed'] and not b['cargo_artifact']['fresh'] and b['binary_sha256']==r['binary_sha256']
assert '109 passed; 0 failed' in (p/'tests.log').read_text()
log=gzip.decompress((p/'js-check.log.gz').read_bytes()).decode();assert 'tests 61' in log and 'pass 61' in log and 'fail 0' in log and 'all six checked gauges are zero' in log
print('Verified panic reproduction, normal CLI error, fresh build, library tests and client cleanup.')
