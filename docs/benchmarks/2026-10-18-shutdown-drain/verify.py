"""Check failure-path regression evidence without launching a relay."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())


build = json.loads((ROOT / 'build.json').read_text())
for source, expected in build['source_sha256'].items():
    assert hashlib.sha256(decoded(Path(source).name)).hexdigest() == expected
before = decoded('shutdown-drain-before.log').decode()
assert '0 passed; 2 failed' in before
assert 'an earlier failure must not abandon a still-running worker' in before
assert 'a failed worker must not bypass the wait for the outer deadline' in before
assert '86 passed; 0 failed' in decoded('shutdown-drain-after.log').decode()
assert '89 passed; 0 failed' in decoded('shutdown-drain-root-tests.log').decode()
assert 'Finished' in decoded('shutdown-drain-clippy.log').decode()
assert decoded('shutdown-drain-fmt.log') == b''
assert 'Finished `release`' in decoded('shutdown-drain-build.log').decode()
for suffix in ['', '-v2']:
    x = json.loads(decoded(f'shutdown-drain-smoke{suffix}.json'))
    assert not x['passed']
    assert x['server_sha256'] == build['binary_sha256']
x = json.loads(decoded('shutdown-drain-smoke-v3.json'))
assert x['passed'] and x['exit_code'] == 0
assert x['server_sha256'] == build['binary_sha256']
assert x['joined_peers'] == len(x['closes']) == 4
assert len({r['id'] for r in x['closes']}) == 4
assert all(r['code'] == 1001 and not r['acknowledged'] for r in x['closes'])
assert x['incomplete_upgrades'] == 1 and x['client_acknowledgements_sent'] == 0
assert x['transport_endings'] == ['eof'] * 5
print('Checksums, selected source identity, before/after regressions, integration checks and release shutdown verified.')
