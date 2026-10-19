import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
def read(name):
    return gzip.decompress((root / 'raw' / (name + '.gz')).read_bytes()).decode()
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and not build['cargo_artifact']['fresh']
assert hashlib.sha256((root / 'raw/legacy_bridge.rs').read_bytes()).hexdigest() == build['source_sha256']['src/legacy_bridge.rs']
before = json.loads(read('bridge-oversize-baseline.json'))
assert not before['passed'] and len(before['cases']) == 2
assert all(not x['passed'] and x['error'] == 'EOFError' for x in before['cases'])
after = json.loads(read('bridge-oversize-selected.json'))
assert after['passed'] and after['bridge_sha256'] == build['binary_sha256']
assert [x['joined'] for x in after['cases']] == [False, True]
assert all(x['passed'] and x['code'] == 1009 and x['header_bytes'] == 14 and x['healthy_native_fifo'] for x in after['cases'])
for x in [before, after]:
    assert x['exit_codes'] == [0, 0] and not any(x['final_gauges'].values())
live = json.loads(read('bridge-oversize-live-check.json'))
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0, 0]
assert live['bridge_sha256'] == build['binary_sha256']
assert 'pass 34' in live['test_output'] and 'fail 0' in live['test_output']
assert not any(live['final_resource_gauges'].values())
assert '91 passed; 0 failed' in read('bridge-oversize-tests.log')
assert 'Finished' in read('bridge-oversize-clippy.log') and 'error:' not in read('bridge-oversize-clippy.log')
print('Original EOF failures, both selected 1009 replies, native witnesses, 91 library and 34 compatibility tests, Clippy and retirement verified.')
