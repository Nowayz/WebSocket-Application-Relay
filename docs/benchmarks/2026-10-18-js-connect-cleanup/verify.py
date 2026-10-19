"""Audit short connection-lifecycle evidence without starting services."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['sources_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes()).decode()


before = decoded('js-connect-before.log')
assert 'pass 0' in before and 'fail 2' in before
assert 'expected: 0' in before and 'actual: 1' in before
after = decoded('js-connect-after.log')
assert 'pass 13' in after and 'fail 0' in after
x = json.loads(decoded('js-connect-live-check.json'))
assert x['passed'] and x['test_exit_code'] == 0 and x['exit_codes'] == [0, 0]
assert x['client_sha256'] == build['sources_sha256']['clients/js/relay.mjs']
assert 'pass 20' in x['test_output'] and 'fail 0' in x['test_output']
assert 'clients/js/connect.test.mjs' in x['test_command']
assert all(value == 0 for value in x['final_resource_gauges'].values())
package = json.loads((ROOT / 'raw/package.json').read_text())
assert 'connect.test.mjs' in package['scripts']['test']
assert 'Done in' in decoded('js-connect-browser-build.log')
print('Checksums, source identities, two original failures, 20 passing client tests, resource gates and browser build verified.')
