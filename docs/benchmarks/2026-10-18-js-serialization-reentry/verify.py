"""Offline audit of recursive serialization rejection and native recovery."""
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

meta = json.loads((root/'builds.json').read_text())
original = read('original-relay.mjs')
selected = read('selected-relay.mjs')
assert selected.replace(b"  if (encoding !== 0) throw new Error('Relay serialization cannot be reentered');\n", b'') == original
original_hash = hashlib.sha256(original).hexdigest()
assert hashlib.sha256(selected).hexdigest() == meta['selected_client_sha256']
assert hashlib.sha256(read('browser-relay.js')).hexdigest() == meta['browser_bundle_sha256']
before = json.loads(read('js-serialization-reentry-before-live-check.json'))
assert not before['passed'] and before['test_exit_code'] == 1
assert before['client_sha256'] == original_hash
assert 'ℹ fail 3' in before['test_output']
for title in ['reentrant application sends fail locally before either packet reaches the socket',
              'a reentrant connect rejects before creating a socket without corrupting the outer packet',
              'native connection remains usable after locally rejected reentrant serialization']:
    assert '✖ ' + title in before['test_output']
after = json.loads(read('js-serialization-reentry-after-live-check.json'))
assert after['passed'] and after['test_exit_code'] == 0 and after['exit_codes'] == [0, 0]
assert after['client_sha256'] == meta['selected_client_sha256']
assert 'ℹ tests 55' in after['test_output'] and 'ℹ pass 55' in after['test_output']
assert len(after['final_resource_gauges']) == 6 and all(v == 0 for v in after['final_resource_gauges'].values())
browser = json.loads(read('js-serialization-reentry-bundle-live-check.json'))
assert browser['passed'] and browser['test_exit_code'] == 0 and browser['exit_codes'] == [0, 0]
assert browser['bundle_sha256'] == meta['browser_bundle_sha256']
assert 'ℹ pass 2' in browser['test_output']
assert all(v == 0 for v in browser['final_resource_gauges'].values())
assert b'globalThis.Buffer = undefined' in read('js-serialization-reentry-bundle.test.mjs')
old_wire = json.loads(read('js-serialization-reentry-old-wire-live-check.json'))
assert old_wire['passed'] and old_wire['test_exit_code'] == 0 and old_wire['exit_codes'] == [0, 0]
assert old_wire['client_sha256'] == original_hash
assert 'original client emits the nested message then receives native protocol Close 1002' in old_wire['test_output']
assert all(v == 0 for v in old_wire['final_resource_gauges'].values())
for other in [before, browser, old_wire]:
    assert (other['server_sha256'], other['bridge_sha256']) == (after['server_sha256'], after['bridge_sha256'])
rows = json.loads(read('js-serialization-reentry-send-screen.json'))
assert [r['variant'] for r in rows] == ['before', 'after', 'after', 'before']
for row in rows:
    assert row['sourceHash'] == (original_hash if row['variant'] == 'before' else meta['selected_client_sha256'])
    assert row['iterations'] == 100000 and row['warmup'] == 20000 and row['payloadBytes'] == 1024
    assert row['encodedBytes'] == 331840000 and len(row['nsPerSend']) == 3
    assert all(0 < value < 10000 for value in row['nsPerSend'])
    assert row['command'][:3] == ['taskset', '-c', '8']
print('Verified one-condition fix, three original failures, old native Close 1002, 55 final passes, browser-module recovery, shared binary identities and source-pinned SDK timing.')
