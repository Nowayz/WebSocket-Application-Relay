"""Audit fail-fast bridge configuration checks without launching services."""
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


assert '0 passed; 1 failed' in decoded('bridge-upstream-before.log')
assert 'invalid upstream accepted: not a URL' in decoded('bridge-upstream-before.log')
assert '92 passed; 0 failed' in decoded('bridge-upstream-tests.log')
assert 'Finished' in decoded('bridge-upstream-clippy.log')
assert decoded('bridge-upstream-fmt.log') == ''
assert 'Finished `release`' in decoded('bridge-upstream-build.log')
cli = json.loads(decoded('bridge-upstream-cli.json'))
assert cli['passed'] and len(cli['cases']) == 10
for case in cli['cases']:
    assert case['server_sha256'] == build[case['variant'] + '_bridge_sha256']
    assert case['exit_code'] != 0
    upstream = case['command'][case['command'].index('--upstream') + 1]
    expected = ('invalid legacy upstream URL' if case['variant'] == 'selected'
                and upstream != 'ws://127.0.0.1:1338' else 'cannot bind legacy bridge')
    assert case['expected_error'] == expected and expected in case['stderr']
live = json.loads(decoded('bridge-upstream-live-check.json'))
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0, 0]
assert live['bridge_sha256'] == build['selected_bridge_sha256']
assert 'pass 21' in live['test_output'] and 'fail 0' in live['test_output']
assert all(value == 0 for value in live['final_resource_gauges'].values())
print('Checksums, source identities, original failure, ten release startup cases, 21 client tests and retirement verified.')
