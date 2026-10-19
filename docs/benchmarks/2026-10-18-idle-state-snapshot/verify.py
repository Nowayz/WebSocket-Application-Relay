"""Audit coherent idle-state acceptance without compiling or serving."""
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
native = json.loads(decoded('native-build.json'))
assert native['passed'] and native['cargo_exit_code'] == 0
assert native['cargo_artifact']['fresh'] is False
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256(decoded(Path(name).name)).hexdigest() == expected
    assert native['source_sha256'][name] == expected
tests = decoded('idle-snapshot-tests.log').decode()
assert '93 passed; 0 failed' in tests
assert 'idle_expiry_preserves_exact_boundary_and_pending_control_deadline ... ok' in tests
assert 'multicast_waits_for_its_owner_while_private_output_stays_immediate ... ok' in tests
assert 'Finished' in decoded('idle-snapshot-clippy.log').decode()
assert decoded('idle-snapshot-fmt.log') == b''
live = json.loads(decoded('idle-snapshot-live-check.json'))
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0, 0]
assert live['server_sha256'] == native['binary_sha256']
assert 'pass 24' in live['test_output'] and 'fail 0' in live['test_output']
assert 'application idle policy reaches native and legacy browser WebSockets cleanly' in live['test_output']
assert all(value == 0 for value in live['final_resource_gauges'].values())
corpus = json.loads(decoded('idle-snapshot-corpus.json'))['case']
assert corpus['passed'] and corpus['corpus_cases'] == 776 and corpus['scheduled_exchanges'] == 2311
assert corpus['server_sha256'] == native['binary_sha256']
print('Checksums, freshly built source identity, controlled interleaving tests, 24 client cases, idle expiry and corpus verified.')
