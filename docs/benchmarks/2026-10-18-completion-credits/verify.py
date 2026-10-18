#!/usr/bin/env python3
"""Verify the rejected credit-refund candidate and both matched screens."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
builds = json.loads((ROOT / 'builds.json').read_text())
rows = json.loads((ROOT / 'summary.json').read_text())
assert builds['selected'] is False and len(rows) == 8
for workload, prefix in [('mixed', 'screen'), ('fanout', 'fanout')]:
    trials = [r for r in rows if r['workload'] == workload]
    assert [r['variant'] for r in trials] == ['control', 'direct', 'direct', 'control']
    for row in trials:
        name = f"credit-batch-{prefix}-{row['trial']}-{row['variant']}.json.gz"
        x = json.loads(gzip.decompress((ROOT / 'raw' / name).read_bytes()))
        assert x['passed'] and x['client_exit_code'] == x['server_exit_code'] == 0
        assert x['server_sha256'] == builds['binaries'][row['variant']]
        assert x['runner_sha256'] == hashlib.sha256((ROOT / 'raw/inspection.py').read_bytes()).hexdigest()
        assert x['args']['pairs'] == (64 if workload == 'mixed' else 0)
        assert x['args']['fanout_window'] == (4 if workload == 'mixed' else 16)
        final = x['events'][-1]
        assert final['event'] == 'done'
        for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                    'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
            assert final['metrics'][key] == 0, (name, key)
        assert final['metrics']['war_channels'] == 1
        m = x['mixed']
        n = m['healthy']['verified'] + m['fanout']['verified']
        assert row['cpu_seconds_per_million'] == m['server_cpu_seconds'] / n * 1e6
        print(name, 'verified')
print('Archive identities, eight successful receipts, unselected status and retirement verified.')
