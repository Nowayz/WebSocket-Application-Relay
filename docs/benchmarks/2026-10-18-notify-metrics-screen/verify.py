#!/usr/bin/env python3
"""Verify diagnostic opportunities and rejected-layout receipts offline."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
builds = json.loads((ROOT / 'builds.json').read_text())
summary = json.loads((ROOT / 'summary.json').read_text())
assert builds['selected'] is False
for path in sorted((ROOT / 'raw').glob('*.json.gz')):
    x = json.loads(gzip.decompress(path.read_bytes()))
    assert x['passed'] and x['client_exit_code'] == x['server_exit_code'] == 0
    assert x['runner_sha256'] == hashlib.sha256((ROOT / 'raw/inspection.py').read_bytes()).hexdigest()
    if path.name.startswith('notify-frequency'):
        assert x['server_sha256'] == builds['diagnostic_sha256']
        a, b = x['events'][:2]
        delta = {key: b['metrics'][key] - a['metrics'][key] for key in summary['notifications']}
        assert delta == summary['notifications']
        assert delta['war_socket_notify_duplicate_total'] == 0
    else:
        variant = 'control' if '-control.' in path.name else 'direct'
        assert x['server_sha256'] == builds['binaries'][variant]
    final = x['events'][-1]
    assert final['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert final['metrics'][key] == 0, (path.name, key)
    assert final['metrics']['war_channels'] == 1
    print(path.name, 'verified')
check = json.loads((ROOT / 'raw/metrics-accounting-check.json').read_text())
assert check['passed'] and check['unauthorized_status'] == 401 and check['authorized_status'] == 200
assert check['queued_bytes'] == check['server_exit_code'] == 0
assert 'global admission' in check['help'] and 'membership snapshots' in check['help']
print('Checksums, opportunities, unselected layouts, admission HELP and retirement verified.')
