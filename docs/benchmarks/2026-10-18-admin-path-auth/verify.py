"""Audit authorization response ordering and live disconnect acceptance offline."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes()).decode()


build = json.loads((ROOT / 'build.json').read_text())
assert hashlib.sha256((ROOT / 'raw/admin.rs').read_bytes()).hexdigest() == build['source_sha256']
assert '0 passed; 1 failed' in decoded('admin-path-before.log')
assert '90 passed; 0 failed' in decoded('admin-path-tests.log')
assert 'Finished' in decoded('admin-path-clippy.log')
assert decoded('admin-path-fmt.log') == ''
assert 'Finished `release`' in decoded('admin-path-build.log')
cases = {}
bad = {'bad', '-1', '18446744073709551616', '%FF'}
for variant in ['control', 'selected']:
    x = json.loads(decoded('admin-path-live-v3-' + variant + '.json'))
    assert x['passed'] and x['exit_code'] == 0
    assert x['server_sha256'] == build[variant + '_binary_sha256']
    assert x['healthy_verified'] == len(x['requests']) == 20
    assert x['administrative_close'] == 4000 and x['departure_verified']
    for row in x['requests']:
        if row['credentials'] == 'valid':
            assert row['status'] == (400 if row['id'] in bad else 404)
        else:
            assert row['status'] == (400 if variant == 'control' and row['id'] in bad else 401)
            if variant == 'selected':
                assert row['body'] == ''
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                'war_routing_copied_bytes_total', 'war_protocol_errors_total']:
        assert x['final_metrics'][key] == 0, key
    cases[variant] = {(r['id'], r['credentials']): r for r in x['requests'] if r['credentials'] == 'valid'}
assert cases['control'] == cases['selected']
print('Checksums, before/after regression, 40 checked disconnect-request cases, 40 healthy deliveries, unchanged authorized responses and resource retirement verified.')
