"""Offline audit of the rejected layout change and selected quota fixtures."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name

def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())

meta = json.loads((root/'builds.json').read_text())
assert not meta['selected_runtime_change'] and meta['selected_tests'] == 4
original = read('original-server.rs')
selected = read('selected-server.rs')
assert selected.replace(b'\n#[cfg(test)]\nmod rate_tests;\n', b'') == original
assert b'rates: [u64; 2],' in original
assert read('rate_tests.rs').count(b'#[test]') == 4
assert b'rate_limiter_bytes=48 connection_bytes=520' in read('shared-rate-layout-before.log')
assert b'rate_limiter_bytes=32 connection_bytes=504' in read('shared-rate-layout-after.log')
builds = {}
for variant in ['control', 'candidate']:
    build = json.loads(read(variant+'-build.json'))
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    assert hashlib.sha256(read('shared-rate-'+variant+'-server.rs')).hexdigest() == build['source_sha256']['src/server.rs']
    builds[variant] = build
assert builds['control']['binary_sha256'] != builds['candidate']['binary_sha256']
rows = json.loads((root/'summary.json').read_text())
assert [r['variant'] for r in rows] == ['control', 'candidate', 'candidate', 'control']
clients = set()
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
    assert x['fanout_windows'] == {'completed': 32, 'offered': 32} and x['fanout_offered_load_met']
    assert x['args']['users'] == 100000 and x['args']['readers'] == 8192 and x['args']['seconds'] == 2
    assert x['args']['churn_workers'] == x['args']['pairs'] == x['args']['inspectors'] == 0
    assert x['sha256']['server'] == builds[row['variant']]['binary_sha256']
    assert x['sha256']['runner'] == hashlib.sha256(read('mixed-capacity.py')).hexdigest()
    clients.add((x['sha256']['capacity_client'], x['sha256']['sidecar']))
    assert row['rss_kib'] == x['before']['rss_kib'] and row['cpu_seconds'] == x['server_cpu_seconds']
    assert row['private_max_ms'] == x['latency']['max_ms']['max']
    assert row['private_p99_user_max_ms'] == x['latency']['max_ms']['p99']
    assert row['fanout_max_ms'] == x['fanout_exact_ms']['max_ms']['max']
    stem = row['receipt'].removesuffix('.json')
    for suffix, key, count, samples, column in [('.csv','csv_sha256',100000,8,'max_ms'), ('-fanout.csv','fanout_csv_sha256',8192,128,'max_ns')]:
        data = read(stem+suffix)
        assert hashlib.sha256(data).hexdigest() == x[key]
        users = list(csv.DictReader(io.StringIO(data.decode())))
        assert len(users) == len({u['id'] for u in users}) == count
        assert all(int(u['samples']) == samples for u in users)
        values = sorted(float(u[column])/(1 if suffix == '.csv' else 1e6) for u in users)
        assert values[-1] == row['private_max_ms' if suffix == '.csv' else 'fanout_max_ms']
        if suffix == '.csv': assert values[(count-1)*99//100] == row['private_p99_user_max_ms']
    final = x['final_metrics']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert final[key] == 0, key
    assert final['war_channels'] == 1
    left = x['events'][-1]['counts']; right = x['sidecar_events'][-1]['details']['counts']
    for metric, field in [('war_accepted_total','joins'), ('war_joined_total','joins'), ('war_disconnected_total','normal_closes'), ('war_incoming_messages_total','sent_messages'), ('war_incoming_bytes_total','sent_bytes'), ('war_deliveries_total','deliveries'), ('war_delivered_bytes_total','delivered_bytes'), ('war_routing_reused_bytes_total','value_bytes')]:
        assert final[metric] == left[field] + right[field], metric
assert len(clients) == 1
controls = [r for r in rows if r['variant'] == 'control']
candidates = [r for r in rows if r['variant'] == 'candidate']
assert min(r['rss_kib'] for r in candidates) > max(r['rss_kib'] for r in controls)
assert min(r['private_max_ms'] for r in candidates) > max(r['private_max_ms'] for r in controls)
small_total = 0
for index, variant in enumerate(['control','candidate','candidate','control']):
    x = json.loads(read(f'shared-rate-small-{index}-{variant}.json'))
    assert x['server_sha256'] == builds[variant]['binary_sha256']
    case = x['cases'][0]
    assert case['passed'] and case['counts_match'] and case['exit_code'] == case['server_exit_code'] == 0
    assert case['duration_seconds'] == 2 and case['clients'] == 64 and case['cleaned_up']
    result = next(v for v in case['output'] if v.get('event') == 'result')
    small_total += result['verified_deliveries']
assert small_total == 13216288
assert b'97 passed; 0 failed' in read('shared-rate-candidate-tests.log')
assert b'96 passed; 0 failed' in read('shared-rate-selected-tests.log')
assert b'Finished' in read('shared-rate-selected-test-clippy.log')
print('Verified fresh paired builds, type layouts, eight short traffic cases, complete 100k/8192 reader samples, exact counters, unfavorable RSS/private maxima and selected test-only policy coverage.')
