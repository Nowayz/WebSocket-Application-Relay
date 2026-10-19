#!/usr/bin/env python3
"""Verify archived evidence without opening sockets or running load."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads(gzip.decompress((ROOT / (name + '.json.gz')).read_bytes()))


def verify():
    for name, expected in json.loads((ROOT / 'sha256.json').read_text()).items():
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    selection = json.loads((ROOT / 'selection.json').read_text())
    build = json.loads((ROOT / 'selected-build.json').read_text())
    assert build['passed'] and build['cargo_exit_code'] == 0
    assert build['binary_sha256'] == selection['sha256']
    for path in ['src/relay.rs', 'src/relay/continuation_tests.rs', 'src/server/socket.rs']:
        body = gzip.decompress((ROOT / ('selected-' + path.replace('/', '-') + '.gz')).read_bytes())
        assert hashlib.sha256(body).hexdigest() == build['source_sha256'][path]
    before = read('wire-before-v2')
    assert not before['passed'] and before['sequence'] == ['unsubscribe_ack', 'late_departure']
    for name in ['wire-final-default', 'wire-final-one-slot-isolated']:
        x = read(name)
        assert x['passed'] and x['audience'] == 100000
        assert x['server_sha256'] == selection['sha256']
        assert x['sequence'] == ['departure', 'unsubscribe_ack']
        assert x['resubscribe_verified'] and x['shutdown_close_verified']
        assert x['server_exit'] == 0 and not x['server_forced_kill']
        assert int(x['after']['war_sessions']) == 100000
        assert int(x['after']['war_queued_bytes']) == 0
        assert int(x['after']['war_routing_copied_bytes_total']) == 0
    for name in ['mixed-room-control', 'mixed-room-final']:
        x = read(name)
        assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
        assert x['fanout_windows'] == {'offered': 16, 'completed': 16}
        m = next(e['details'] for e in x['sidecar_events'] if e['event'] == 'measured')
        assert m['fanout']['verified'] == 524288 and m['churn']['verified'] == 512
        assert x['events'][-1]['counts']['departure_checks'] == 750000
        for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                    'war_read_buffer_reserved_bytes', 'war_native_connection_slots',
                    'war_routing_copied_bytes_total']:
            assert int(x['final_metrics'][key]) == 0, (name, key)
    tests = gzip.decompress((ROOT / 'final-tests.log.gz').read_bytes()).decode()
    assert '147 passed; 0 failed' in tests
    print('PASS: archive integrity, exact source/build identity, ordering, resubscribe, mixed delivery and cleanup')


if __name__ == '__main__':
    verify()
