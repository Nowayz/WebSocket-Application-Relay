import ast, gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert hashlib.sha256(gzip.decompress((root / 'original-inspection.py.gz').read_bytes())).hexdigest() == meta['original_source_sha256']
source = (root / 'inspection.py').read_bytes()
assert hashlib.sha256(source).hexdigest() == meta['selected_source_sha256']
assert hashlib.sha256((root / 'test-inspection-cleanup.py').read_bytes()).hexdigest() == meta['test_source_sha256']
ast.parse(source)
ast.parse((root / 'test-inspection-cleanup.py').read_bytes())
before = (root / 'before-v2.log').read_text()
assert 'TimeoutExpired' in before and 'errors=2' in before
assert 'Ran 1 test' in (root / 'tests.log').read_text() and 'OK' in (root / 'tests.log').read_text()
assert 'Ran 8 tests' in (root / 'receipt-tests.log').read_text() and 'OK' in (root / 'receipt-tests.log').read_text()
probe = json.loads((root / 'probe.json').read_text())
assert probe['passed'] and probe['cli_exit_code'] == 1 and probe['fallback_killed_pids'] == []
assert all(x['exit_code'] is not None for x in probe['children_before_fallback'])
stuck = json.loads(gzip.decompress((root / 'stuck.json.gz').read_bytes()))
normal = json.loads(gzip.decompress((root / 'real-profile.json.gz').read_bytes()))
assert not stuck['passed'] and stuck['perf_forced_kill'] and stuck['perf_exit_code'] == stuck['client_exit_code'] == -9
assert stuck['server_exit_code'] == 0 and 'controlled perf ignores SIGINT' in stuck['perf_log']
assert normal['passed'] and not normal['perf_forced_kill'] and normal['perf_exit_code'] == -2
assert normal['client_exit_code'] == normal['server_exit_code'] == 0
for p in [stuck, normal]:
    assert p['profile_diagnostic'] and p['args']['seconds'] == 1
    assert p['runner_sha256'] == meta['selected_source_sha256']
for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']:
    assert normal['events'][-1]['metrics'][key] == 0
print('PASS: original cleanup failures; controlled single grace/reap; real children all retired; real-perf compatibility and failed receipt preserved')
