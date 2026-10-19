import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
for name, expected in json.loads((root / 'build.json').read_text())['source_sha256'].items():
    assert hashlib.sha256((root / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected
before = json.loads(gzip.decompress((root / 'raw/listen-flags-before-check.json.gz').read_bytes()))
assert not before['passed'] and 'InvalidAccessError' in before['test_output']
final = json.loads(gzip.decompress((root / 'raw/listen-flags-final-check.json.gz').read_bytes()))
assert final['passed'] and final['test_exit_code'] == 0
assert 'pass 34' in final['test_output'] and 'fail 0' in final['test_output']
assert final['exit_codes'] == [0, 0]
assert all(v == 0 for v in final['final_resource_gauges'].values())
print('Source hashes, original failure, 34 passing tests and resource retirement verified.')
queue = json.loads(gzip.decompress((root / 'raw/js-queue-boundary-check.json.gz').read_bytes()))
assert queue['passed'] and queue['test_exit_code'] == 0 and queue['exit_codes'] == [0, 0]
assert 'pass 34' in queue['test_output'] and 'fail 0' in queue['test_output']
assert all(v == 0 for v in queue['final_resource_gauges'].values())
print('Real-transport queued command failure boundary verified.')
