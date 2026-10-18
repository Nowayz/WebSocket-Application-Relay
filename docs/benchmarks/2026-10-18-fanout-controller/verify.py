#!/usr/bin/env python3
"""Check archived diagnostic clocks, delivery matrices and retirement receipts."""
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

for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert sha((ROOT / name).read_bytes()) == expected, name

summary = json.loads((ROOT / 'summary.json').read_text())
for trial in summary['trials']:
    name = trial['label']
    record = json.loads(decoded(name + '.json'))
    assert record['passed'] and record['exit_codes'] == [0, 0, 0]
    assert record['fanout_offered_load_met']
    for suffix, field in [('.csv', 'csv_sha256'), ('-fanout.csv', 'fanout_csv_sha256'),
                          ('.windows.json', 'latency_windows_sha256')]:
        if field not in record:
            continue
        data = decoded(name + suffix)
        assert sha(data) == record[field]
        if suffix.endswith('.csv'):
            rows = list(csv.DictReader(io.StringIO(data.decode())))
            readers = record['args']['readers'] if suffix.startswith('-') else record['args']['users']
            assert len(rows) == len({r['id'] for r in rows}) == readers
            samples = (record['fanout_windows']['completed'] * record['args']['window']
                       if suffix.startswith('-') else record['args']['seconds'] * 4)
            assert all(int(r['samples']) == samples for r in rows)
    timing = record['sidecar_events'][1]['details']['fanout_controller_timing']
    if timing is not None:
        assert len(timing['windows']) == record['fanout_windows']['completed']
        for row in timing['windows']:
            assert row['scheduled_ns'] <= row['wake_ns'] <= row['sent_ns'] <= row['acknowledged_ns'] <= row['completed_ns']
    else:
        assert name == 'fanout-batch-off-smoke'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert record['final_metrics'][key] == 0
    assert record['final_metrics']['war_channels'] == 1
    print(name, 'verified')
print('Archive hashes, decoded data, stage ordering, samples and retirement verified.')
