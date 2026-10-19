"""Audit live pressure separately from teardown; never starts a server."""
import gzip
import hashlib
import json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name

def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())

rows = json.loads((root/'summary.json').read_text())
assert len(rows) == 9
identities = set()
seconds = 0
for row in rows:
    receipt = json.loads(read(row['receipt']))
    identities.add((receipt['server_sha256'], receipt['stress_sha256']))
    case = next(c for c in receipt['cases'] if c['mode'] == row['mode'])
    output = next(e for e in case['output'] if e.get('event') == 'result')
    assert case['passed'] and case['counts_match'] and case['pressure_valid'] and row['passed']
    assert case['exit_code'] == case['server_exit_code'] == 0 and not case['server_forced_kill']
    assert case['cleaned_up'] and not case['admin_errors']
    assert output['verified_deliveries'] == row['verified_deliveries'] > 0
    assert output['latency_max_ms'] == row['latency_max_ms']
    seconds += case['duration_seconds']
    metrics = case['final']['metrics']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']:
        assert int(metrics[key]) == 0, (row['receipt'], key)
    assert int(metrics['war_channels']) == 1
    if case['mode'] == 'fanout':
        assert int(metrics['war_routing_reused_bytes_total']) + int(metrics['war_routing_copied_bytes_total']) == output['sent_value_bytes']
    if row['gated']:
        assert '--teardown-gate' in case['command']
        assert [e['event'] for e in case['output']] == ['ready', 'traffic_finished', 'result']
        live = case['traffic_finished']['metrics']
        for key, value in row['live'].items():
            assert int(live[key]) == value
        unread = output['unread_clients']
        healthy = case['clients'] - unread
        assert int(live['war_slow_consumers_total']) == unread
        assert int(live['war_sessions']) == healthy
        assert healthy <= int(live['war_connections']) <= case['clients']
        assert output['verified_deliveries'] == output['sent_messages'] * output['healthy_readers']
        assert output['disconnect_notifications'] == unread * output['healthy_readers']
        assert output['teardown_capacity_closes'] == 0
        assert len(output['unread_observations']) == unread
assert seconds == 16 and len(identities) == 1
assert {'traffic', 'churn', 'slow', 'fanout'} == {r['mode'] for r in rows}
confirmation = next(r for r in rows if r['receipt'] == 'short-pressure-gated-confirmation.json')
assert confirmation['verified_deliveries'] == 1355420
assert confirmation['live'] == {'war_sessions': 62, 'war_connections': 62, 'war_slow_consumers_total': 4, 'war_queued_bytes': 0}
fixed = next(r for r in rows if r['receipt'] == 'short-pressure-gated-1k.json')
assert fixed['live']['war_connections'] == 64 and fixed['live']['war_sessions'] == 62
assert '4 tests' in read('short-pressure-gate-final-tests.log').decode()
assert 'OK' in read('short-pressure-gate-final-tests.log').decode()
print('Verified nine short cases, live overload/membership gates, pending-Close distinction, healthy control, shared binaries, all-mode compatibility and final resource retirement.')
