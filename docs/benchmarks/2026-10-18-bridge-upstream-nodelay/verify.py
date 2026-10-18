#!/usr/bin/env python3
"""Offline audit of the one-flag transport change and its actual test receipts."""
import hashlib
import json
import lzma
from pathlib import Path
import statistics
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
def read(name):
    return (HERE / name).read_bytes()
def load(name):
    return json.loads(read(name))
def digest(data):
    return hashlib.sha256(data).hexdigest()
for name, expected in load('manifest.json').items():
    assert digest(read(name)) == expected, name
for name, expected in load('decoded-sha256.json').items():
    assert digest(lzma.decompress(read(name))) == expected, name
for dep in load('dependencies.json'):
    assert digest((ROOT / dep['path']).read_bytes()) == dep['sha256']
builds = {row['label']: row for row in load('artifacts/bridge-upstream-nodelay-build.json')}
for label, row in builds.items():
    prefix = 'artifacts/candidates/bridge-upstream-nodelay-' + label
    assert digest(lzma.decompress(read(prefix + '/war-legacy-bridge.xz'))) == row['binary_sha256']
    assert digest(read(prefix + '/legacy_bridge.rs')) == row['source_sha256']
control = read('artifacts/candidates/bridge-upstream-nodelay-control/legacy_bridge.rs').decode()
candidate = control.replace('accept_hdr_async_with_config, connect_async,', 'accept_hdr_async_with_config, connect_async_with_config,', 1).replace(
    '    let (mut upstream, _) = timeout(WAIT, connect_async(upstream)).await??;',
    "    // Forward small realtime packets without waiting for the prior write's ACK.\n    let (mut upstream, _) =\n        timeout(WAIT, connect_async_with_config(upstream, None, true)).await??;", 1)
assert candidate == read('artifacts/candidates/bridge-upstream-nodelay-candidate/legacy_bridge.rs').decode()
selected_source = candidate.replace("// Forward small realtime packets without waiting for the prior write's ACK.", "// Disable Nagle's ACK-dependent delay for consecutive small relay packets.")
assert selected_source == read('sources/legacy_bridge.rs').decode()
selected = load('artifacts/candidates/bridge-upstream-nodelay-selected/build.json')
assert selected['source_revision'] == 'db798cee46055f1d6766d1d0e158c4f0e602fb41'
assert digest(read('sources/legacy_bridge.rs')) == selected['legacy_bridge_sha256']
assert digest(read('sources/Cargo.lock')) == selected['cargo_lock_sha256']
assert digest(lzma.decompress(read('artifacts/candidates/bridge-upstream-nodelay-selected/war-legacy-bridge.xz'))) == selected['binary_sha256']
connector = read('dependency-source/connect.rs').decode()
assert 'connect_async_with_config(request, None, false).await' in connector
assert 'if disable_nagle {' in connector and 'socket.set_nodelay(true)?' in connector

def probe(case, expected_hash):
    assert case['passed'] and case['exit_code'] == 0 and not case['fixture_errors']
    assert case['bridge_sha256'] == expected_hash
    assert len(case['trials']) == 8
    assert [t['sequence'] for t in case['trials']] == list(range(8))
    for t in case['trials']:
        assert 0 <= t['first_forward_ms'] and 0 <= t['second_forward_ms'] <= t['arrival_gap_ms']
    return [t['second_forward_ms'] for t in case['trials']]

campaign = load('artifacts/bridge-upstream-nodelay-campaign.json')
assert campaign['passed'] and [r['label'] for r in campaign['runs']] == ['control', 'candidate', 'candidate', 'control']
timings = {'control':[], 'candidate':[]}
for run in campaign['runs']:
    case = load(run['receipt'])
    values = probe(case, builds[run['label']]['binary_sha256'])
    assert statistics.median(values) == run['median_second_forward_ms']
    assert max(values) == run['max_second_forward_ms']
    timings[run['label']].extend(values)
assert len(timings['control']) == len(timings['candidate']) == 16
assert max(timings['candidate']) < min(timings['control'])
values = probe(load('artifacts/bridge-upstream-nodelay-selected-probe.json'), selected['binary_sha256'])
assert max(values) < min(timings['control'])
probe(load('artifacts/bridge-nagle-baseline-probe.json'), '8e8b5c771bebcf9ef055b3db8ff1c986521303211ecdf6e338df2b1b4223fc72')
for suffix, count, skipped in [('js-check', 6, 1), ('js-idle-check', 7, 0)]:
    case = load('artifacts/bridge-upstream-nodelay-' + suffix + '.json')
    assert case['passed'] and case['test_exit_code'] == 0 and case['exit_codes'] == [0, 0]
    assert case['bridge_sha256'] == selected['binary_sha256']
    assert case['server_sha256'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
    assert f'pass {count}' in case['test_output'] and f'skipped {skipped}' in case['test_output']
    assert all(value == 0 for value in case['final_resource_gauges'].values())
assert '78 passed; 0 failed' in read('artifacts/bridge-upstream-nodelay-clean-tests.log').decode()
assert '81 passed; 0 failed' in read('artifacts/bridge-upstream-nodelay-workspace-tests.log').decode()
assert 'Finished' in read('artifacts/bridge-upstream-nodelay-clippy.log').decode()
assert not read('artifacts/bridge-upstream-nodelay-format.log')
adoption = load('artifacts/bridge-upstream-nodelay-adoption.json')
metadata = load('manual-services.json')
assert adoption['passed'] and not adoption.get('rollback_pid')
assert adoption['bridge_sha256'] == metadata['bridge_sha256'] == selected['binary_sha256']
assert adoption['source_revision'] == metadata['bridge_revision'] == selected['source_revision']
assert adoption['new_bridge_pid'] == metadata['pids'][1]
assert adoption['native_pid_unchanged'] == metadata['pids'][0]
assert adoption['static_pid_unchanged'] == metadata['pids'][2]
assert all(value == 0 for value in adoption['final_resource_gauges'].values())
assert len(adoption['verified']) == 5
print('PASS: exact connector delta; all binary/source pins; 32 matched delayed-ACK trials plus final/preliminary probes; observed latency separation; clean/workspace tests; seven JS/idle tests; guarded live adoption and retirement receipt')
