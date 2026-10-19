#!/usr/bin/env python3
"""Audit pre-welcome control handling and actual capacity-close evidence offline."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name

def read(name):
    return gzip.decompress((ROOT / (name + '.gz')).read_bytes())

old, new = read('original-legacy_bridge.rs'), read('selected-legacy_bridge.rs')
old_prod, new_prod = old.split(b'#[cfg(test)]\nmod tests')[0], new.split(b'#[cfg(test)]\nmod tests')[0]
assert old_prod[:old_prod.index(b'    let welcome = timeout')] == new_prod[:new_prod.index(b'    let welcome = timeout')]
assert old_prod[old_prod.index(b'    let id = Decoder::new(&welcome)'):] == new_prod[new_prod.index(b'    let id = Decoder::new(&welcome)'):]
assert b'let welcome = timeout(WAIT, async {' in new_prod
assert b'Message::Ping(_) => upstream.flush().await?' in new_prod
assert b'Message::Pong(_) => {}' in new_prod
assert b'transport::initiate_close(&mut legacy, frame, WAIT, None).await;' in new_prod
builds = {v:json.loads((ROOT / (v + '-build.json')).read_text()) for v in ['control', 'selected']}
assert builds['selected']['passed'] and builds['selected']['cargo_artifact']['fresh'] is False
for variant, source in [('control', old), ('selected', new)]:
    assert hashlib.sha256(source).hexdigest() == builds[variant]['source_sha256']['src/legacy_bridge.rs']
assert json.loads((ROOT / 'source-differences.json').read_text()) == ['src/legacy_bridge.rs']
assert '1 passed; 2 failed' in (ROOT / 'bridge-welcome-before-tests.log').read_text()
assert '104 passed; 0 failed' in (ROOT / 'bridge-welcome-selected-tests.log').read_text()
assert 'Finished' in (ROOT / 'bridge-welcome-clippy.log').read_text()
for variant in ['original', 'selected']:
    record=json.loads((ROOT / ('bridge-welcome-native-' + variant + '-v2.json')).read_text())
    assert record['passed'] and record['test_exit_code'] == 0 and record['exit_codes'] == [0,0]
    assert record['before_fds'] == record['after_fds'] == [18,10]
    build = builds['control' if variant == 'original' else 'selected']
    assert record['bridge_sha256'] == build['binary_sha256']
    assert record['test_sha256'] == hashlib.sha256((ROOT / 'bridge-welcome-native.test.mjs').read_bytes()).hexdigest()
    event=json.loads(record['test_output'])
    assert event['native_rejections'] == 2 and event['replacement_routed']
    assert len(event['outcomes']) == 2
    for outcome in event['outcomes']:
        assert outcome['messages'] == 0
        assert outcome['code'] == (1006 if variant == 'original' else 1013)
        assert outcome['reason'] == ('' if variant == 'original' else 'Try Again Later')
        assert outcome['clean'] == (variant == 'selected')
    metrics=record['final_metrics']
    assert metrics['war_accepted_total'] == 4
    assert metrics['war_joined_total'] == metrics['war_disconnected_total'] == 2
    assert metrics['war_join_rejected_total'] == metrics['war_rejected_total'] == 2
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:
        assert metrics[key] == 0
for variant in ['original','selected']:
    preliminary=json.loads((ROOT / ('bridge-welcome-native-' + variant + '.json')).read_text())
    assert not preliminary['passed'] and 'test_exit_code' not in preliminary
    assert preliminary['exit_codes'] == [0,0]
js=json.loads((ROOT / 'bridge-welcome-js-live-check.json').read_text())
assert js['passed'] and js['test_exit_code'] == 0 and js['exit_codes'] == [0,0]
assert js['bridge_sha256'] == builds['selected']['binary_sha256']
assert 'pass 61' in js['test_output'] and 'fail 0' in js['test_output']
assert all(v == 0 for v in js['final_resource_gauges'].values())
print('PASS: isolated welcome-phase change, fresh build, 104 tests, exact 1006-to-1013 transition, slot/FD reuse and 61 compatibility tests')
