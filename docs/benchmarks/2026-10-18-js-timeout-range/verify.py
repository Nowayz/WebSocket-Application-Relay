"""Audit request-timeout range validation without starting services."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes()).decode()


before = decoded('js-timeout-before.log')
assert 'pass 0' in before and 'fail 3' in before
assert 'TimeoutOverflowWarning' in before and 'Timeout duration was set to 1' in before
after = decoded('js-timeout-after.log')
assert 'pass 21' in after and 'fail 0' in after
assert 'TimeoutOverflowWarning' not in after
live = json.loads(decoded('js-timeout-live-check.json'))
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0, 0]
assert live['client_sha256'] == build['source_sha256']['clients/js/relay.mjs']
assert 'pass 28' in live['test_output'] and 'fail 0' in live['test_output']
assert all(value == 0 for value in live['final_resource_gauges'].values())
assert 'Done in' in decoded('js-timeout-browser-build.log')
print('Checksums, source identities, three original failures, timer overflow evidence, 28 passing client tests and retirement verified.')
