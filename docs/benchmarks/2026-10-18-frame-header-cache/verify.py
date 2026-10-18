#!/usr/bin/env python3
"""Verify the rejected cache experiment's immutable receipts and retirement."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
builds = json.loads((ROOT / 'builds.json').read_text())
assert builds['selected'] is False
summary = json.loads((ROOT / 'summary.json').read_text())
assert [row['variant'] for row in summary] == ['control', 'direct', 'direct', 'control']
for row in summary:
    name = f"frame-header-screen-{row['trial']}-{row['variant']}.json.gz"
    x = json.loads(gzip.decompress((ROOT / 'raw' / name).read_bytes()))
    assert x['passed'] and x['client_exit_code'] == x['server_exit_code'] == 0
    assert x['server_sha256'] == builds['binaries'][row['variant']]
    assert x['runner_sha256'] == hashlib.sha256((ROOT / 'raw/inspection.py').read_bytes()).hexdigest()
    final = x['events'][-1]
    assert final['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert final['metrics'][key] == 0, (name, key)
    assert final['metrics']['war_channels'] == 1
    m = x['mixed']
    deliveries = m['healthy']['verified'] + m['fanout']['verified']
    assert row['cpu_seconds_per_million_deliveries'] == m['server_cpu_seconds'] / deliveries * 1e6
    print(name, 'verified')
print('Checksums, variant identities, selected=false and all retirement gates verified.')
