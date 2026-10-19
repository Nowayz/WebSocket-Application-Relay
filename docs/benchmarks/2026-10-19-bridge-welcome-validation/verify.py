#!/usr/bin/env python3
"""Audit complete welcome validation and retained protocol test evidence offline."""
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
marker=b'    legacy\n        .send(Message::Binary(Bytes::copy_from_slice(&id.to_le_bytes())))'
assert old_prod[old_prod.index(marker):] == new_prod[new_prod.index(marker):]
assert b'id != 0 && id != BROADCAST && decoder.position() == bytes.len()' in new_prod
assert b'    drop(welcome);\n    let Some(id) = id else' in new_prod
assert b'Invalid Upstream Welcome' in new_prod and b'tokio::join!(' in new_prod
builds={v:json.loads((ROOT / (v+'-build.json')).read_text()) for v in ['control','selected']}
for v,data in [('control',old),('selected',new)]:
    assert hashlib.sha256(data).hexdigest() == builds[v]['source_sha256']['src/legacy_bridge.rs']
assert builds['selected']['passed'] and builds['selected']['cargo_artifact']['fresh'] is False
assert json.loads((ROOT / 'source-differences.json').read_text()) == ['src/legacy_bridge.rs']
failure=(ROOT / 'bridge-welcome-id-original-ids-tests.log').read_text()
assert 'invalid welcome was exposed as a legacy ID' in failure
assert 'Binary(b"\\0\\0\\0\\0\\0\\0\\0\\0")' in failure
assert 'FAILED. 0 passed; 1 failed' in failure
initial=(ROOT / 'bridge-welcome-id-selected-tests.log').read_text()
assert '105 passed; 1 failed' in initial and 'ConnectionReset' in initial
assert '106 passed; 0 failed' in (ROOT / 'bridge-welcome-id-final-tests-v3.log').read_text()
assert 'Finished' in (ROOT / 'bridge-welcome-id-clippy-v3.log').read_text()
assert b'Message::Text("42".into())' in new
assert b'Message::Binary(Bytes::from_static(&[0x1b, 0, 0, 0, 0, 0, 0, 0, 42]))' in new
js=json.loads((ROOT / 'bridge-welcome-validation-js-live-check.json').read_text())
assert js['passed'] and js['test_exit_code'] == 0 and js['exit_codes'] == [0,0]
assert js['bridge_sha256'] == builds['selected']['binary_sha256']
assert 'pass 61' in js['test_output'] and 'fail 0' in js['test_output']
assert js['final_resource_gauges'] and all(v == 0 for v in js['final_resource_gauges'].values())
print('PASS: reserved-zero reproduction, exact parser guards and ownership, unchanged forwarding, fresh build, 106 tests and 61 compatibility checks')
