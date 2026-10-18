#!/usr/bin/env python3
"""Verify archived identities, finite samples, cadence and retirement receipts."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent

def digest(data):
    return hashlib.sha256(data).hexdigest()

def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())

for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert digest((ROOT / name).read_bytes()) == expected, name

summary = json.loads((ROOT / 'summary.json').read_text())
for trial in summary:
    label = trial['label']
    prefix = 'page-integration-' + label
    record = json.loads(decoded(prefix + '.json'))
    assert record['passed'] and record['exit_codes'] == [0, 0, 0]
    runner = 'page-integration-original-mixed-runner.py' if label.startswith('100k-') else 'mixed-capacity.py'
    assert digest((ROOT / 'raw' / runner).read_bytes()) == record['sha256']['runner']
    rows_by_kind = {}
    for suffix, field in [('.csv', 'csv_sha256'), ('-fanout.csv', 'fanout_csv_sha256'),
                          ('.windows.json', 'latency_windows_sha256')]:
        data = decoded(prefix + suffix)
        assert digest(data) == record[field]
        if suffix.endswith('.csv'):
            rows_by_kind[suffix] = list(csv.DictReader(io.StringIO(data.decode())))
    rows = rows_by_kind['.csv']
    users, samples = record['args']['users'], record['args']['seconds'] * 4
    assert len(rows) == len({row['id'] for row in rows}) == users
    assert all(int(row['samples']) == samples for row in rows)
    assert sum(int(row['samples']) for row in rows) == record['latency_sampling']['total_private_observations']
    for column in ['p99_ms', 'max_ms', 'schedule_p99_ms', 'schedule_max_ms']:
        values = sorted(float(row[column]) for row in rows)
        for name, q in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]:
            assert values[(len(values) - 1) * q // 100] == record['latency'][column][name]
    audience = rows_by_kind['-fanout.csv']
    windows = record['fanout_windows']
    readers = record['args']['readers']
    assert len(audience) == len({row['id'] for row in audience}) == readers
    assert all(int(row['samples']) == windows['completed'] * record['args']['window'] for row in audience)
    if 'fanout_offered_load_met' in record:
        assert record['fanout_offered_load_met'] == (windows['completed'] == windows['offered'])
    final = record['final_metrics']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert final[key] == 0, (label, key)
    assert final['war_channels'] == 1
    print(label, 'verified;', windows['completed'], '/', windows['offered'], 'fanout windows')
print('Archive hashes, decoded receipts, samples, quantiles and final gauges verified.')
