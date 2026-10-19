"""Verify wire-ID validation evidence and component timing medians offline."""
import hashlib
import json
import statistics
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
s = json.loads((ROOT / 'summary.json').read_text())
assert s['selected']
assert hashlib.sha256((ROOT / 'raw/client.mjs').read_bytes()).hexdigest() == s['client_sha256']
live = json.loads((ROOT / 'raw/js-wire-id-check.json').read_text())
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0,0]
assert live['client_sha256'] == s['client_sha256']
assert 'tests 12' in live['test_output'] and 'fail 0' in live['test_output']
assert all(value == 0 for value in live['final_resource_gauges'].values())
assert 'fail 0' in (ROOT / 'raw/js-ids-after.log').read_text()
assert 'fail 2' in (ROOT / 'raw/js-ids-before.log').read_text()
for row in s['cost']:
    d = json.loads((ROOT / 'raw' / row['file']).read_text())
    assert d['verified'] == 720000 and len(d['ns_per_message']) == 7
    assert statistics.median(d['ns_per_message']) == row['median_ns']
print('Checksums, reproduced failures, selected tests, live compatibility and timing medians verified.')
