#!/usr/bin/env python3
"""Recompute reservation retry deltas and verify finite fixture gates."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'builds.json').read_text())
assert build['selected'] is False
rows = json.loads((ROOT / 'summary.json').read_text())
for row in rows:
    data = json.loads(gzip.decompress((ROOT / 'raw' / (row['case'] + '.json.gz')).read_bytes()))
    assert data['passed'] and not data['server_forced_kill']
    assert data['client_exit_code'] == data['server_exit_code'] == 0
    assert data['server_sha256'] == build['diagnostic_sha256']
    assert data['runner_sha256'] == hashlib.sha256((ROOT / 'raw/inspection.py').read_bytes()).hexdigest()
    a, b, end = data['events']
    assert [a['event'], b['event'], end['event']] == ['ready', 'measured', 'done']
    for key, value in row.items():
        if key != 'case':
            assert b['metrics'][key] - a['metrics'][key] == value
    assert row['war_admit_probe_local_calls_total'] == row['war_admit_probe_global_calls_total'] > 0
    assert row['war_admit_probe_local_retries_total'] == row['war_admit_probe_untracked_total'] == 0
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert end['metrics'][key] == 0, (row['case'], key)
    assert end['metrics']['war_channels'] == 1
    print(row['case'], 'verified')
print('All checksums, retry deltas and fixture retirement gates verified.')
