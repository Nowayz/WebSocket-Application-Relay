"""Audit prompt local-close cancellation without starting services."""
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


before = decoded('js-local-close-before.log')
assert 'pass 0' in before and 'fail 1' in before
assert 'local close rejects pending and queued commands' in before
after = decoded('js-local-close-after.log')
assert 'pass 14' in after and 'fail 0' in after
x = json.loads(decoded('js-local-close-live-check.json'))
assert x['passed'] and x['test_exit_code'] == 0 and x['exit_codes'] == [0, 0]
assert x['client_sha256'] == build['source_sha256']['clients/js/relay.mjs']
assert 'pass 21' in x['test_output'] and 'fail 0' in x['test_output']
assert all(value == 0 for value in x['final_resource_gauges'].values())
assert 'Done in' in decoded('js-local-close-browser-build.log')
print('Checksums, exact source identities, original failure, 21 passing client tests, resource retirement and browser build verified.')
