#!/usr/bin/env python3
"""Audit client cleanup evidence without starting services or requiring Node."""
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

original, selected = read('original-relay.mjs'), read('selected-relay.mjs')
assert original[original.index(b'  static async connect'):] == selected[selected.index(b'  static async connect'):]
a = original.index(b'      try {', original.index(b'constructor(ws'))
b = selected.index(b'      try {', selected.index(b'constructor(ws'))
assert original[a:original.index(b"\n    });\n    ws.addEventListener('close'", a)] == selected[b:selected.index(b'\n    };\n    const closed = event', b)]
assert b"ws.removeEventListener('message', message);" in selected
assert b"ws.removeEventListener('close', closed);" in selected
build = json.loads((ROOT / 'build.json').read_text())
assert hashlib.sha256(selected).hexdigest() == build['source_sha256']
assert hashlib.sha256(read('browser-relay.js')).hexdigest() == build['bundle_sha256']
before = json.loads((ROOT / 'js-final-close-before-live-check.json').read_text())
assert not before['passed'] and before['test_exit_code'] != 0 and before['exit_codes'] == [0, 0]
assert before['client_sha256'] == hashlib.sha256(original).hexdigest()
assert '1 !== 0' in before['test_output'] and 'native final close' in before['test_output']
assert 'fail 2' in (ROOT / 'js-final-close-before-mock.log').read_text()
assert 'pass 22' in (ROOT / 'js-final-close-after-mock.log').read_text()
for stem, count in [('js-final-close-final-live-check', 58), ('js-final-close-bundle-live-check', 4)]:
    record = json.loads((ROOT / (stem + '.json')).read_text())
    assert record['passed'] and record['test_exit_code'] == 0 and record['exit_codes'] == [0, 0]
    assert record['client_sha256'] == build['source_sha256']
    assert f'pass {count}' in record['test_output'] and 'fail 0' in record['test_output']
    assert record['final_resource_gauges'] and all(v == 0 for v in record['final_resource_gauges'].values())
    assert record['server_sha256'] == before['server_sha256']
    assert record['bridge_sha256'] == before['bridge_sha256']
    if 'bundle' in stem:
        assert record['bundle_sha256'] == build['bundle_sha256']
assert b'getEventListeners(native.ws' in read('selected-relay.test.mjs')
print('PASS: reproduced retention, unchanged message/command paths, 58 source and four bundle checks, clean resource/process receipts')
