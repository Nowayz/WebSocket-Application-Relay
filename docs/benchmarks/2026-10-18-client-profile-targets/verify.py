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
assert len(rows) == 4
assert [r['variant'] for r in rows] == ['control', 'candidate', 'candidate', 'control']
server_hashes = {}
clients = set()
identities = set()
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
    assert x['passed'] == row['overall_passed'] == x['fanout_offered_load_met']
    assert x['fanout_windows'] == row['windows']
    assert row['windows']['offered'] == 32
    assert x['args']['users'] == 100000 and x['args']['readers'] == 8192 and x['args']['seconds'] == 2
    assert x['args']['churn_workers'] == x['args']['inspectors'] == x['args']['pairs'] == 0
    assert row['server_cpu_seconds'] == x['server_cpu_seconds']
    assert row['private_max_ms'] == x['latency']['max_ms']['max']
    assert row['fanout_max_ms'] == x['fanout_exact_ms']['max_ms']['max']
    identities.add(tuple(x['sha256'][k] for k in ['server', 'capacity_client', 'sidecar']))
    server_hashes.setdefault(row['variant'], x['sha256']['server'])
    assert server_hashes[row['variant']] == x['sha256']['server']
    clients.add((x['sha256']['capacity_client'], x['sha256']['sidecar']))
    runner = 'runner.py'
    assert hashlib.sha256(read(runner).encode()).hexdigest() == x['sha256']['runner']
    assert all(m['process']['fds'] is None for m in x['monitoring'])
    assert isinstance(x['before']['fds'], int) and isinstance(x['after']['fds'], int)
    assert x['fanout_window_timing']['windows'] == row['windows']['completed']
    stem = row['receipt'].removesuffix('.json')
    for suffix, key, count, samples in [('.csv', 'csv_sha256', 100000, 8), ('-fanout.csv', 'fanout_csv_sha256', 8192, row['windows']['completed'] * 4)]:
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
    if x['args']['churn_workers'] == 0:
        ready = next(e for e in x['sidecar_events'] if e['event'] == 'ready')
        measured = next(e for e in x['sidecar_events'] if e['event'] == 'measured')
        assert right['joins'] == ready['details']['counts']['joins']
        assert measured['details']['churn']['verified'] == 0
assert len(identities) == 2 and len(clients) == 2
assert len(set(server_hashes.values())) == 1
assert [r['overall_passed'] for r in rows] == [True, True, False, True]
assert [r['windows']['completed'] for r in rows] == [32, 32, 31, 32]
earlier = json.loads(read('earlier-builds.json'))
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['sha256']['sidecar'] == earlier[row['variant'] + '_client_sha256']
for target, name in [('sidecar', 'fanout-sidecar-profile.json'), ('server', 'profile-target-server-live.json'), ('capacity', 'profile-target-capacity-live.json')]:
    x = json.loads(read(name))
    assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
    assert x['args']['profile'] and x['args']['profile_target'] == target
    profile = x['profile']
    assert profile['target'] == target
    command = profile['command']
    assert command[command.index('-p') + 1] == str(profile['pid'])
    assert command[command.index('-o') + 1].endswith('.perf.data')
    assert hashlib.sha256(read('runner.py').encode()).hexdigest() == x['sha256']['runner']
assert 'war-relay' in read('profile-target-server-dso.txt')
assert 'capacity' in read('profile-target-capacity-dso.txt')
assert 'Total Lost Samples: 0' in read('fanout-sidecar-profile-symbols.txt')
assert '21.85%' in read('fanout-sidecar-profile-symbols.txt')
for case in json.loads(read('profile-target-cli-tests.json')):
    assert case['exit_code'] == 2 and not case['output_created']
for name in ['profile-target-output-tests.log', 'profile-target-timing-tests.log']:
    assert 'OK' in read(name)
print('Verified four corrected-monitor client comparisons, missed-load failure, complete-population samples, exact counters, all three live profile targets and argument/collision checks.')
