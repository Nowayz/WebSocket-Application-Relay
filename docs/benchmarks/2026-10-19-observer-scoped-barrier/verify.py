#!/usr/bin/env python3
"""Verify the scoped-barrier evidence offline; run no traffic."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads(gzip.decompress((ROOT / (name + '.json.gz')).read_bytes()))


def summarize():
    build = json.loads((ROOT / 'selected-build.json').read_text())
    control = json.loads((ROOT / 'control-build.json').read_text())
    assert build['passed'] and control['passed']
    selection = json.loads((ROOT / 'selection.json').read_text())
    assert selection['passed'] and selection['sha256'] == build['binary_sha256']
    for path in ['src/relay.rs', 'src/relay/continuation_tests.rs']:
        for label, receipt in [('base', control), ('selected', build)]:
            body = gzip.decompress((ROOT / (label + '-' + path.replace('/', '-') + '.gz')).read_bytes())
            assert hashlib.sha256(body).hexdigest() == receipt['source_sha256'][path]
    fixture_sha = hashlib.sha256(gzip.decompress((ROOT / 'wire-check.py.gz').read_bytes())).hexdigest()
    rows = []
    for name, kind in [('wire-1-control', 'control'), ('wire-2-candidate', 'candidate'),
                       ('wire-3-candidate', 'candidate'), ('wire-4-control', 'control'),
                       ('wire-one-slot-candidate', 'candidate')]:
        x = read(name)
        assert x['passed'] and x['audience'] == 100000 and x.get('error') is None
        assert x['fixture_sha256'] == fixture_sha
        assert x['server_sha256'] == (control if kind == 'control' else build)['binary_sha256']
        assert x['sequence'] == ['departure', 'unsubscribe_ack'] and x['resubscribe_verified']
        assert x['server_exit'] == 0 and not x['server_forced_kill'] and x['shutdown_close_verified']
        assert int(x['after']['war_sessions']) == 100000
        assert int(x['after']['war_queued_bytes']) == 0
        assert int(x['after']['war_routing_copied_bytes_total']) == 0
        samples = x['probe_samples']
        overlap = [s for s in samples if s['received_ns'] >= x['barrier_start_ns']
                   and s['sent_ns'] <= x['barrier_end_ns']]
        assert len(overlap) == x['probe_overlap_count']
        maximum = max(((s['received_ns'] - s['sent_ns']) / 1e6 for s in overlap), default=None)
        assert maximum == x['probe_overlap_max_ms']
        if 'one-slot' not in name:
            assert overlap
        assert (x['barrier_end_ns'] - x['barrier_start_ns']) / 1e6 == x['unsubscribe_ack_ms']
        rows.append({'name': name, 'ack_ms': x['unsubscribe_ack_ms'], 'witness_overlap_ms': maximum})
    mixed = []
    for name, receipt in [('mixed-control', control), ('mixed-candidate-retry', build)]:
        x = read(name)
        assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
        assert x['sha256']['server'] == receipt['binary_sha256']
        assert x['fanout_windows'] == {'offered': 16, 'completed': 16}
        m = next(e['details'] for e in x['sidecar_events'] if e['event'] == 'measured')
        assert m['fanout']['verified'] == 524288 and m['churn']['verified'] == 512
        assert x['events'][-1]['counts']['departure_checks'] == 750000
        for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                    'war_read_buffer_reserved_bytes', 'war_native_connection_slots',
                    'war_routing_copied_bytes_total']:
            assert int(x['final_metrics'][key]) == 0, (name, key)
        mixed.append({'name': name, 'cpu_seconds': x['server_cpu_seconds'],
                      'private_per_second': m['healthy']['per_second'],
                      'worst_capacity_user_ms': x['latency']['max_ms']['max'],
                      'fanout_max_ms': m['fanout']['max_ms']})
    failed = read('mixed-candidate')
    assert not failed['passed'] and not failed['sidecar_events']
    log = gzip.decompress((ROOT / 'mixed-candidate-sidecar.err.gz').read_bytes()).decode()
    assert 'Cannot assign requested address' in log
    before = gzip.decompress((ROOT / 'before.log.gz').read_bytes()).decode()
    after = gzip.decompress((ROOT / 'final-tests.log.gz').read_bytes()).decode()
    assert 'ordinary room work must remain deferred' in before and '1 failed' in before
    assert '147 passed; 0 failed' in after
    return {'wire': rows, 'mixed': mixed}


if __name__ == '__main__':
    for name, expected in json.loads((ROOT / 'sha256.json').read_text()).items():
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    result = summarize()
    assert result == json.loads((ROOT / 'summary.json').read_text())
    print(json.dumps(result, indent=2))
    print('PASS: source/build identity, event ordering, witness timing, mixed traffic and cleanup')
