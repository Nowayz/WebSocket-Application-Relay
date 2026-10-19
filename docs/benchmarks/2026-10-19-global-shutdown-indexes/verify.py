#!/usr/bin/env python3
"""Check the archived shutdown evidence without running a server."""
import gzip
import hashlib
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads(gzip.decompress((ROOT / (name + '.json.gz')).read_bytes()))


def summarize():
    builds = {kind: json.loads((ROOT / (kind + '-build.json')).read_text())
              for kind in ['control', 'selected']}
    assert all(x['passed'] and x['cargo_exit_code'] == 0 for x in builds.values())
    for path in ['src/relay.rs', 'src/relay/continuation_tests.rs']:
        for label, kind in [('base', 'control'), ('selected', 'selected')]:
            body = gzip.decompress((ROOT / (label + '-' + path.replace('/', '-') + '.gz')).read_bytes())
            assert hashlib.sha256(body).hexdigest() == builds[kind]['source_sha256'][path]
    selection = json.loads((ROOT / 'selection.json').read_text())
    assert selection['passed'] and selection['sha256'] == builds['selected']['binary_sha256']
    fixture_sha = hashlib.sha256(gzip.decompress((ROOT / 'shutdown-wire-default.py.gz').read_bytes())).hexdigest()
    rows = []
    for name, kind in [('default-1-candidate', 'selected'), ('default-2-control', 'control'),
                       ('default-3-control', 'control'), ('default-4-candidate', 'selected')]:
        x = read(name)
        assert x['passed'] and x.get('error') is None and x.get('shutdown_error') is None
        assert x['fixture_sha256'] == fixture_sha and x['mode'] == 'global_observers'
        assert x['server_sha256'] == builds[kind]['binary_sha256']
        assert x['audience'] == x['shutdown_closes'] == 100000
        assert x['shutdown_close_verified'] and x['server_exit'] == 0 and not x['server_forced_kill']
        assert x['command'][x['command'].index('--shutdown-timeout-secs') + 1] == '10'
        assert x['sequence'] == ['departure', 'unsubscribe_ack'] and x['resubscribe_verified']
        assert int(x['after']['war_sessions']) == 100000
        assert int(x['after']['war_queued_bytes']) == 0
        assert int(x['after']['war_routing_copied_bytes_total']) == 0
        rows.append({'name': name, 'shutdown_check_ms': x['shutdown_check_elapsed_ms'],
                     'shutdown_closes': x['shutdown_closes'], 'unsubscribe_ack_ms': x['unsubscribe_ack_ms']})
    early = read('global-1-control')
    assert not early['passed'] and early['server_exit'] == 1 and not early['server_forced_kill']
    assert 'service shutdown exceeded its deadline' in early['server_log']
    tight = read('shutdown-2-candidate')
    assert not tight['passed'] and tight['server_exit'] == 0 and not tight['server_forced_kill']
    assert tight['shutdown_closes'] == 72846 and 'unexpected WebSocket EOF' in tight['shutdown_error']
    before = gzip.decompress((ROOT / 'shutdown-before.log.gz').read_bytes()).decode()
    tests = gzip.decompress((ROOT / 'shutdown-final-tests.log.gz').read_bytes()).decode()
    assert '1 failed' in before and '148 passed; 0 failed' in tests
    old = statistics.median(r['shutdown_check_ms'] for r in rows if 'control' in r['name'])
    new = statistics.median(r['shutdown_check_ms'] for r in rows if 'candidate' in r['name'])
    return {'default_grace_runs': rows, 'control_median_ms': old, 'selected_median_ms': new,
            'cleanup_reduction_percent': (1 - new / old) * 100,
            'two_second_candidate_closes': tight['shutdown_closes']}


if __name__ == '__main__':
    for name, sha in json.loads((ROOT / 'sha256.json').read_text()).items():
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == sha, name
    result = summarize()
    assert result == json.loads((ROOT / 'summary.json').read_text())
    print(json.dumps(result, indent=2))
    print('PASS: archived build/source identity, full 100k Close matrix, timing and negative controls')
