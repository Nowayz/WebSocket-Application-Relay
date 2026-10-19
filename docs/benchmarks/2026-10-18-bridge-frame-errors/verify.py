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
before = json.loads(read('bridge-frame-errors-baseline.json'))
after = json.loads(read('bridge-frame-errors-after.json'))
assert not before['passed'] and sum(not c['passed'] for c in before['cases']) == 8
assert all(c['passed'] if c['kind'] == 'oversize' else c['error'] == 'EOFError' for c in before['cases'])
assert after['passed'] and after['bridge_sha256'] == build['binary_sha256']
expected = {'oversize':1009, 'unmasked':1002, 'reserved-opcode':1002, 'fragmented-ping':1002, 'invalid-utf8':1007}
for x in [before, after]:
    assert len(x['cases']) == 10
    assert {(c['joined'],c['kind']) for c in x['cases']} == {(joined,kind) for joined in [False, True] for kind in expected}
    assert all(c['healthy_native_fifo'] and 3 <= c['header_bytes'] <= 14 for c in x['cases'])
    assert x['exit_codes'] == [0,0] and not any(x['final_gauges'].values())
assert all(c['passed'] and c['opcode'] == 8 and c['code'] == expected[c['kind']] for c in after['cases'])
live = json.loads(read('bridge-frame-errors-live-check.json'))
assert live['passed'] and live['test_exit_code'] == 0 and live['exit_codes'] == [0,0]
assert live['bridge_sha256'] == build['binary_sha256']
assert 'pass 34' in live['test_output'] and 'fail 0' in live['test_output']
assert not any(live['final_resource_gauges'].values())
assert '91 passed; 0 failed' in read('bridge-frame-errors-tests.log')
assert 'Finished' in read('bridge-frame-errors-clippy.log') and 'error:' not in read('bridge-frame-errors-clippy.log')
print('Eight original failures, ten selected Close receipts/native witnesses, 91 library and 34 compatibility tests, Clippy and resource retirement verified.')
