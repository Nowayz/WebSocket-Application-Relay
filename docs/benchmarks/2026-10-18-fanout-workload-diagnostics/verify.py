"""Offline evidence audit; does not start servers or load generators."""
import csv, gzip, hashlib, io, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name

def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()

rows = json.loads((root/'summary.json').read_text())
assert len(rows) == 8
assert [r['churn_workers'] for r in rows[:4]] == [8, 0, 0, 8]
assert [r['timed_fd_scan'] for r in rows[4:]] == [False, True, True, False]
identities = set()
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
    assert x['fanout_windows'] == {'completed': 32, 'offered': 32}
    assert x['fanout_offered_load_met']
    assert x['args']['users'] == 100000 and x['args']['readers'] == 8192 and x['args']['seconds'] == 2
    assert x['args']['churn_workers'] == row['churn_workers']
    assert row['cpu_seconds'] == x['server_cpu_seconds']
    assert row['private_max_ms'] == x['latency']['max_ms']['max']
    assert row['fanout_max_ms'] == x['fanout_exact_ms']['max_ms']['max']
    identities.add(tuple(x['sha256'][k] for k in ['server', 'capacity_client', 'sidecar']))
    runner = 'mixed-capacity-with-fd-scans.py' if row['timed_fd_scan'] else 'mixed-capacity.py'
    assert hashlib.sha256(read(runner).encode()).hexdigest() == x['sha256']['runner']
    assert all((m['process']['fds'] is not None) == row['timed_fd_scan'] for m in x['monitoring'])
    assert isinstance(x['before']['fds'], int) and isinstance(x['after']['fds'], int)
    assert x['fanout_window_timing']['windows'] == 32
    stem = row['receipt'].removesuffix('.json')
    for suffix, key, count, samples in [('.csv', 'csv_sha256', 100000, 8), ('-fanout.csv', 'fanout_csv_sha256', 8192, 128)]:
        text = read(stem+suffix)
        assert hashlib.sha256(text.encode()).hexdigest() == x[key]
        users = list(csv.DictReader(io.StringIO(text)))
        assert len(users) == len({u['id'] for u in users}) == count
        assert all(int(u['samples']) == samples for u in users)
        column = 'max_ms' if suffix == '.csv' else 'max_ns'
        assert max(float(u[column]) for u in users)/(1 if suffix == '.csv' else 1e6) == row['private_max_ms' if suffix == '.csv' else 'fanout_max_ms']
    final = x['final_metrics']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total']:
        assert final[key] == 0, key
    assert final['war_channels'] == 1
    left = x['events'][-1]['counts']; right = x['sidecar_events'][-1]['details']['counts']
    for metric, field in [('war_accepted_total','joins'), ('war_joined_total','joins'), ('war_disconnected_total','normal_closes'), ('war_incoming_messages_total','sent_messages'), ('war_incoming_bytes_total','sent_bytes'), ('war_deliveries_total','deliveries'), ('war_delivered_bytes_total','delivered_bytes'), ('war_routing_reused_bytes_total','value_bytes')]:
        assert final[metric] == left[field] + right[field], metric
    if row['churn_workers'] == 0:
        ready = next(e for e in x['sidecar_events'] if e['event'] == 'ready')
        measured = next(e for e in x['sidecar_events'] if e['event'] == 'measured')
        assert right['joins'] == ready['details']['counts']['joins']
        assert measured['details']['churn']['verified'] == 0
assert len(identities) == 1
calibration = json.loads(read('fd-snapshot-cost.json'))
assert calibration['child_exit'] == 0 and len(calibration['rows']) == 6
for name in ['fanout-diagnostics-gate-tests.log', 'fanout-diagnostics-unit-tests.log', 'fanout-diagnostics-output-tests.log']:
    assert 'OK' in read(name)
print('Verified eight complete 100k-user receipts, 6.4M private observations, 8.39M fanout observations, exact counters, shared binaries, observer variants and regression evidence.')
