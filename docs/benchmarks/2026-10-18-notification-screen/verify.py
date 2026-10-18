#!/usr/bin/env python3
"""Check experiment provenance, source isolation, placement and final integrations."""
import ast
import hashlib
import json
import lzma
from pathlib import Path
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
def read(name):
    return (HERE / name).read_bytes()
def load(name):
    return json.loads(read(name))
def digest(data):
    return hashlib.sha256(data).hexdigest()
def function(name):
    tree = ast.parse(read(name).decode())
    return next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'pin_child_workers')
for name, expected in load('manifest.json').items():
    assert digest(read(name)) == expected, name
for name, expected in load('decoded-sha256.json').items():
    assert digest(lzma.decompress(read(name))) == expected, name
for dep in load('dependencies.json'):
    assert digest((ROOT / dep['path']).read_bytes()) == dep['sha256']
assert ast.dump(function('sources/benchmark.py'), include_attributes=False) == ast.dump(function('artifacts/notify-guard-pinning-helper-before.py'), include_attributes=False)
assert 'pin_child_workers = runtime.pin_child_workers' in read('sources/capacity.py').decode()
builds = {row['label']: row for row in load('artifacts/notify-guard-build.json')}
for label, row in builds.items():
    prefix = 'artifacts/candidates/notify-guard-' + label
    assert digest(lzma.decompress(read(prefix + '/war-relay.xz'))) == row['binary_sha256']
    assert digest(read(prefix + '/socket.rs')) == row['socket_source_sha256']
control = read('artifacts/candidates/notify-guard-control/socket.rs').decode()
candidate = control.replace('use parking_lot::Mutex;', 'use parking_lot::{Mutex, MutexGuard};', 1).replace(
    '    fn notify(&self) {\n        if !self.notified.swap(true, Relaxed) {',
    "    // Producers hold this socket's State lock. The owner clears the flag\n    // before driving under that same lock: it either consumes this submission\n    // or leaves a queued wake. Producer serialization needs no atomic RMW.\n    fn notify(&self, _state: &MutexGuard<'_, State>) {\n        if !self.notified.load(Relaxed) {\n            self.notified.store(true, Relaxed);", 1).replace('self.notify();', 'self.notify(&state);')
assert read('artifacts/candidates/notify-guard-candidate/socket.rs').decode() == candidate
position = candidate.rfind('\n}')
assert read('sources/candidate-tested-socket.rs').decode() == candidate[:position] + '\n' + read('artifacts/notify-guard-tests.txt').decode() + candidate[position:]
assert '80 passed; 0 failed' in read('artifacts/notify-guard-tests.log').decode()

def counters(case):
    assert case['passed'] and case['client_exit_code'] == case['server_exit_code'] == 0
    assert not case.get('server_forced_kill')
    for event in case['events']:
        assert all(event['metrics'][key] == value for key,value in event['exact_counter_checks'].items())
        if 'worker_read_allowance_bound' in event:
            assert 0 <= event['metrics']['war_read_buffer_reserved_bytes'] <= event['worker_read_allowance_bound']
    assert case['events'][-1]['metrics']['war_connections'] == 0
    assert case['events'][-1]['metrics']['war_queued_bytes'] == 0

total = 0
for pinned in [False, True]:
    campaign = load('artifacts/notify-guard-' + ('pinned-' if pinned else '') + 'campaign.json')
    assert campaign['passed'] and len(campaign['runs']) == 4
    for run in campaign['runs']:
        assert run['exit_code'] == 0
        case = load(run['receipt'])
        counters(case)
        assert not case['profile_diagnostic']
        assert case['args']['seconds'] == 4 and case['args']['fanout_readers'] == 4096 and case['args']['fanout_window'] == 4
        assert case['server_sha256'] == builds[run['label']]['binary_sha256']
        assert case['client_sha256'] == 'ad34e266cd2fb6cb69f270caff448764bdb4f173953ea1d4ac723043ee730de0'
        runner = 'artifacts/notify-guard-inspection-pinned.py' if pinned else 'artifacts/notify-guard-inspection-before.py'
        assert case['runner_sha256'] == digest(read(runner))
        n = case['mixed']['fanout']['verified']
        assert n > 0 and n % (4096*4) == 0
        total += n
        if pinned:
            assert case['pinning_helper_sha256'] == digest(read('artifacts/notify-guard-pinning-helper-before.py'))
            assert len(case['thread_affinity']) == 12
            assert {pin['cpu'] for pin in case['thread_affinity']} == set(range(12))
assert total == 33325056
profile = load('artifacts/fanout-refresh-profile.json')
counters(profile)
assert profile['server_sha256'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
assert profile['client_sha256'] == 'ad34e266cd2fb6cb69f270caff448764bdb4f173953ea1d4ac723043ee730de0'
assert profile['profile_diagnostic'] and '6159 samples' in profile['perf_log']
assert 'Total Lost Samples: 0' in read('artifacts/fanout-refresh-profile-self.txt').decode()
for label, source in [('final-inspection-smoke', 'sources/inspection.py'), ('default-inspection-smoke', 'sources/inspection.py'), ('final-capacity-smoke', 'sources/capacity.py')]:
    case = load('artifacts/notify-guard-' + label + '.json')
    counters(case)
    assert case['runner_sha256'] == digest(read(source))
    assert case['server_sha256'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
    if label == 'default-inspection-smoke':
        assert not case['thread_affinity'] and '--pin-workers' not in case['server_command']
    else:
        assert '--pin-workers' in case['server_command']
        pins = case['thread_affinity']
        assert len(pins) == 12
        assert {p['cpu'] for p in pins if p['method'] == 'server_startup'} == set(range(4))
        assert {p['cpu'] for p in pins if p['method'] == 'harness'} == set(range(4,12))
assert '--pin-client-workers requires --client-workers' in read('artifacts/notify-guard-pin-cli-error.log').decode()
print('PASS: all source/binary pins; eight complete fanout trials/33,325,056 deliveries; profile isolation; shared helper identity; verified affinities/default behavior; final integration and CLI evidence')
