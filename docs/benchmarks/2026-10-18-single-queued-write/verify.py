#!/usr/bin/env python3
"""Verify queue-shape evidence, native variants, exact populations and retirement."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

def sha(data):
    return hashlib.sha256(data).hexdigest()

def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())

def retired(metrics):
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total']:
        assert metrics[key] == 0, key
    assert metrics['war_channels'] == 1

for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert sha((ROOT / name).read_bytes()) == expected, name
builds = json.loads((ROOT / 'builds.json').read_text())
summary = json.loads((ROOT / 'summary.json').read_text())
assert builds['selected_source_commit'] == '266e5b1' and builds['selected_variant'] == 'outline'
for name, label in [('write-shape-mixed', 'completion'), ('write-shape-queue', 'queue')]:
    x = json.loads(decoded(name + '.json'))
    assert x['passed'] and x['server_sha256'] == builds['diagnostics'][label]
    a, b = x['events'][:2]
    h = [b['metrics'][f'war_write_shape_{i}_total'] - a['metrics'][f'war_write_shape_{i}_total'] for i in range(17)]
    if label == 'completion':
        assert sum(h) == 1530833 and h[1] == 1529762
    else:
        q = [b['metrics'][f'war_queue_shape_{i}_total'] - a['metrics'][f'war_queue_shape_{i}_total'] for i in range(17)]
        assert q == h and sum(q) == q[1] == 1155072
    retired(x['events'][-1]['metrics'])
for row in summary['short']:
    x = json.loads(decoded(row['file']))
    assert x['passed'] and x['client_exit_code'] == x['server_exit_code'] == 0
    assert x['server_sha256'] == builds['binaries'][row['variant']]
    assert x['runner_sha256'] == sha((ROOT / 'raw/inspection.py').read_bytes())
    m = x['mixed'];n = m['healthy']['verified'] + m['fanout']['verified']
    assert row['cpu_seconds_per_million'] == m['server_cpu_seconds'] / n * 1e6
    retired(x['events'][-1]['metrics'])
assert len(summary['short']) == 16
for label in ['control', 'direct', 'outline']:
    prefix = 'single-write-100k-' + label
    x = json.loads(decoded(prefix + '.json'))
    assert x['passed'] and x['exit_codes'] == [0, 0, 0] and x['fanout_offered_load_met']
    assert x['fanout_windows'] == {'completed': 96, 'offered': 96}
    assert x['sha256']['server'] == builds['binaries'][label]
    assert x['sha256']['runner'] == sha((ROOT / 'raw/mixed-capacity.py').read_bytes())
    for suffix, field in [('.csv', 'csv_sha256'), ('-fanout.csv', 'fanout_csv_sha256'),
                          ('.windows.json', 'latency_windows_sha256')]:
        data = decoded(prefix + suffix)
        assert sha(data) == x[field]
        if suffix.endswith('.csv'):
            rows = list(csv.DictReader(io.StringIO(data.decode())))
            audience = suffix.startswith('-')
            assert len(rows) == len({r['id'] for r in rows}) == (8192 if audience else 100000)
            assert all(int(r['samples']) == (384 if audience else 24) for r in rows)
            if not audience:
                for column in ['p99_ms', 'max_ms', 'schedule_p99_ms', 'schedule_max_ms']:
                    values = sorted(float(r[column]) for r in rows)
                    for name, q in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]:
                        assert values[(len(values) - 1) * q // 100] == x['latency'][column][name]
    retired(x['final_metrics'])
    print(prefix, 'verified; 96/96 windows')
print('Archive identities, two probes, 16 short trials, all matrices/quantiles and retirement verified.')
