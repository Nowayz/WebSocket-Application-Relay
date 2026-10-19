import argparse
import csv
import gzip
import hashlib
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--check-local-csv', action='store_true')
args = p.parse_args()
root = Path(__file__).resolve().parent
repo = root.parents[2]
meta = json.loads((root / 'provenance.json').read_text())
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
builds = {name: json.loads((root / (name + '-build.json')).read_text()) for name in ['control', 'selected', 'diagnostic']}
for b in builds.values():
    assert b['passed'] and b['cargo_artifact']['fresh'] is False
for source, digest in meta['selected_source_sha256'].items():
    assert hashlib.sha256((root / Path(source).name).read_bytes()).hexdigest() == digest
    assert builds['selected']['source_sha256'][source] == digest
assert builds['control']['source_sha256']['src/server.rs'] == hashlib.sha256(gzip.decompress((root / 'original-server.rs.gz').read_bytes())).hexdigest()
assert builds['diagnostic']['source_sha256']['src/server.rs'] == hashlib.sha256(gzip.decompress((root / 'diagnostic-server.rs.gz').read_bytes())).hexdigest()
assert len({b['source_sha256']['src/relay.rs'] for b in builds.values()}) == 1
assert builds['control']['binary_sha256'] != builds['selected']['binary_sha256']
source = (root / 'server.rs').read_text()
assert 'const READY_QUANTUM: usize = 1024;' in source
assert 'for _ in 0..READY_QUANTUM' in source
assert 'self.schedule_timers(now);' in source
assert 'self.ready.len().min(READY_QUANTUM)' in source
assert '1 failed' in (root / 'before-tests.log').read_text()
assert '120 passed; 1 failed' in (root / 'stale-tests.log').read_text()
assert 'Compiling war-relay' in (root / 'tests.log').read_text()
assert '121 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
assert '65' in (root / 'js.log').read_text() and 'all six checked gauges are zero' in (root / 'js.log').read_text()


def receipt(name):
    raw = gzip.decompress((root / (name + '.json.gz')).read_bytes())
    return raw, json.loads(raw)


def retired(m):
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_native_connection_slots', 'war_routing_copied_bytes_total']:
        assert m[key] == 0, (key, m[key])

_, diagnostic = receipt('target-diagnostic')
assert diagnostic['passed'] and diagnostic['diagnostic_only']
assert diagnostic['binary_sha256'] == builds['diagnostic']['binary_sha256']
assert len(diagnostic['timer_bursts']) == 8
assert sum(row[1] for row in diagnostic['timer_bursts']) == 100000
assert all(row[2] > 12000 for row in diagnostic['timer_bursts'])
comparison = json.loads((root / 'comparison.json').read_text())
assert comparison['passed'] and [r['kind'] for r in comparison['runs']] == ['control', 'selected', 'selected', 'control']
for row in comparison['runs']:
    raw, x = receipt('v4-' + row['label'])
    assert hashlib.sha256(raw).hexdigest() == row['receipt_sha256']
    assert x['passed'] and not x['diagnostic_only'] and not x['timer_bursts']
    assert x['server_exit_code'] == 0 and not x.get('forced_kill')
    assert x['binary_sha256'] == builds[row['kind']]['binary_sha256']
    assert x['controller_sha256'] == hashlib.sha256((root / 'expiry-pressure.py').read_bytes()).hexdigest()
    assert x['pending']['war_connections'] == 100001
    assert x['pending']['war_handshake_errors_total'] == x['pending']['war_rejected_total'] == 0
    assert x['ramp_seconds'] < 3 and x['suspend_seconds'] >= 4.05
    assert x['final']['war_handshake_errors_total'] == 100000
    assert x['final']['war_accepted_total'] == 100001 and x['final']['war_incoming_messages_total'] == 3
    assert x['final']['war_joined_total'] == x['final']['war_disconnected_total'] == 1
    assert x['baseline_fd_count'] == x['final_fd_count']
    retired(x['final'])
    assert row['first_post_resume_probe_ms'] == x['first_post_resume_probe_ms']
    print(f"{row['label']}: {x['first_post_resume_probe_ms']:.3f} ms first verified post-resume echo")
for name in ['control', 'selected', 'selected-repeat', 'control-repeat']:
    _, x = receipt('normal-' + name)
    kind = 'control' if name.startswith('control') else 'selected'
    assert x['passed'] and x['correctness_passed'] and x['fanout_offered_load_met']
    assert x['sha256']['server'] == builds[kind]['binary_sha256']
    assert x['exit_codes'] == [0, 0, 0]
    assert x['fanout_windows'] == {'completed': 32, 'offered': 32}
    assert x['latency_sampling']['users'] == 100000
    assert x['latency_sampling']['observations_per_user'] == 8
    assert x['latency_sampling']['total_private_observations'] == 800000
    measured = next(e for e in x['events'] if e['event'] == 'measured')
    assert measured['details']['counts']['deliveries'] == 800000
    sidecar = next(e for e in x['sidecar_events'] if e['event'] == 'measured')
    assert sidecar['details']['churn']['verified'] == 256
    assert sidecar['details']['fanout']['verified'] == 1048576
    retired(x['final_metrics'])
    if args.check_local_csv:
        prefix = repo / Path(x['args']['output']).with_suffix('')
        for suffix, digest, count, samples in [('.csv', x['csv_sha256'], 100000, 8), ('-fanout.csv', x['fanout_csv_sha256'], 8192, 128)]:
            path = Path(str(prefix) + suffix)
            assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
            with path.open() as f:
                rows = list(csv.DictReader(f))
            assert len(rows) == len({r['id'] for r in rows}) == count
            assert all(int(r['samples']) == samples for r in rows)
            if suffix == '.csv':
                assert all(r['p99_ms'] == r['max_ms'] for r in rows)
                assert max(float(r['max_ms']) for r in rows) == x['latency']['max_ms']['max']
            else:
                assert max(int(r['max_ns']) for r in rows) / 1e6 == x['fanout_exact_ms']['max_ms']['max']
    print(f"normal {name}: all private/fanout deliveries, 32 windows, 256 churn lifecycles and retirement verified")
for name in ['diagnostic', 'control-1', 'v3-selected-1']:
    _, x = receipt(name)
    assert not x['passed'] and x['server_exit_code'] == 0
_, failed_gate = receipt('v2-selected-1')
assert not failed_gate['passed'] and failed_gate['final']['war_connections'] == 0
assert failed_gate['final']['war_native_connection_slots'] == 256
print('Expiry fairness selection verified; failed fixtures and stale debug result remain excluded.')
