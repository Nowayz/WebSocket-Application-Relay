import ast, gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for file, digest in meta['selected_source_sha256'].items():
    data = (root / Path(file).name).read_bytes()
    assert hashlib.sha256(data).hexdigest() == digest
    ast.parse(data)
assert 'errors=4' in (root / 'before.log').read_text()
assert 'JSONDecodeError' in (root / 'before.log').read_text()
assert 'Ran 8 tests' in (root / 'tests-final.log').read_text() and 'OK' in (root / 'tests-final.log').read_text()
assert 'Ran 2 tests' in (root / 'mixed-tests.log').read_text() and 'OK' in (root / 'mixed-tests.log').read_text()
cases = json.loads((root / 'cases.json').read_text())
assert len(cases) == 4 and all(x['returncode'] == 1 and not x['client_started'] for x in cases)
zero = ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']
for runner in ['inspection', 'capacity']:
    for suffix, error in [('permission', 'PermissionError'), ('interpreter', 'FileNotFoundError')]:
        p = json.loads(gzip.decompress((root / f'{runner}-{suffix}.json.gz').read_bytes()))
        assert not p['passed'] and p['server_exit_code'] is None and error in p['error']
        assert p['server_log'] == p['client_log'] == '' and not p['events']
    p = json.loads(gzip.decompress((root / f'{runner}.json.gz').read_bytes()))
    assert p['passed'] and p['server_exit_code'] == p['client_exit_code'] == 0
    assert p['args']['seconds'] == 1 and not p['profile_diagnostic']
    assert p['events'][-1]['event'] == 'done'
    for key in zero:
        assert p['events'][-1]['metrics'][key] == 0
    for event in p['events']:
        for key, value in event.get('exact_counter_checks', {}).items():
            assert event['metrics'][key] == value
print('PASS: syntax/source hashes; four original empty-receipt failures; four recorded OS errors; two ordinary workload/retirement checks')
