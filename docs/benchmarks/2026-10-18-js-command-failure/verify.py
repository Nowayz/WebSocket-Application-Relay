"""Verify immediate command-failure regression and compatibility evidence."""
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'builds.json').read_text())
x = json.loads((ROOT / 'raw/js-pending-failure-check.json').read_text())
assert build['selected'] and x['client_sha256'] == build['client_sha256']
assert x['passed'] and x['test_exit_code'] == 0 and x['exit_codes'] == [0,0]
assert 'tests 13' in x['test_output'] and 'fail 0' in x['test_output']
assert all(v == 0 for v in x['final_resource_gauges'].values())
before = (ROOT / 'raw/js-pending-failure-before.log').read_text()
after = (ROOT / 'raw/js-pending-failure-after.log').read_text()
assert 'command waited for Close or its request timeout' in before and 'fail 1' in before
assert 'tests 6' in after and 'fail 0' in after
print('Checksums, reproduced delay, immediate rejection and live compatibility verified.')
