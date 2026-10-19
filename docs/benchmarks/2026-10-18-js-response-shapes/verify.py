"""Verify response-shape rejection, compatibility and component evidence."""
import hashlib
import json
import statistics
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
s = json.loads((ROOT / 'summary.json').read_text())
x = json.loads((ROOT / 'raw/js-shape-check.json').read_text())
assert s['selected'] and x['client_sha256'] == s['client_sha256']
assert x['passed'] and x['exit_codes'] == [0,0] and x['test_exit_code'] == 0
assert 'tests 16' in x['test_output'] and 'fail 0' in x['test_output']
assert all(v == 0 for v in x['final_resource_gauges'].values())
assert 'fail 2' in (ROOT / 'raw/js-shape-before.log').read_text()
after = (ROOT / 'raw/js-shape-after.log').read_text()
assert 'tests 9' in after and 'fail 0' in after
for row in s['cost']:
    d = json.loads((ROOT / 'raw' / row['file']).read_text())
    assert d['verified'] == 720000 and len(d['ns_per_message']) == 7
    assert statistics.median(d['ns_per_message']) == row['median_ns']
print('Checksums, reproduced shape failures, selected tests and timing medians verified.')
