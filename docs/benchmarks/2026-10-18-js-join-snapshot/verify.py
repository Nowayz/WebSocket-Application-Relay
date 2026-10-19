"""Offline audit of channel capture regressions and generated-module acceptance."""
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
assert hashlib.sha256(selected).hexdigest() == meta['selected_client_sha256']
assert hashlib.sha256(read('browser-relay.js')).hexdigest() == meta['browser_bundle_sha256']
inserted = b'    // Capture the channel before asynchronous socket setup lets its owner reuse it.\n    const subscription = encoder.encode(channel);\n'
assert selected.count(inserted) == 1
assert selected.replace(inserted, b'').replace(b'ws.send(subscription)', b'ws.send(encoder.encode(channel))') == original
before = json.loads(read('js-join-snapshot-before-live-check.json'))
assert not before['passed'] and before['test_exit_code'] == 1
assert before['client_sha256'] == hashlib.sha256(original).hexdigest()
for title in ['connect captures a byte channel before the delayed socket open',
              'connect does not depend on the channel buffer remaining attached',
              'native subscription retains the byte channel supplied before the socket opens']:
    assert '✖ ' + title in before['test_output']
assert 'ℹ fail 3' in before['test_output']
after = json.loads(read('js-join-snapshot-final-live-check.json'))
assert after['passed'] and after['test_exit_code'] == 0 and after['exit_codes'] == [0, 0]
assert after['client_sha256'] == meta['selected_client_sha256']
assert 'ℹ tests 44' in after['test_output'] and 'ℹ pass 44' in after['test_output'] and 'ℹ fail 0' in after['test_output']
assert all(v == 0 for v in after['final_resource_gauges'].values())
assert len(after['final_resource_gauges']) == 6
browser = json.loads(read('js-join-snapshot-browser-path-live-check.json'))
assert browser['passed'] and browser['test_exit_code'] == 0 and browser['exit_codes'] == [0, 0]
assert browser['bundle_sha256'] == meta['browser_bundle_sha256']
assert 'ℹ pass 1' in browser['test_output']
assert all(v == 0 for v in browser['final_resource_gauges'].values())
assert (before['server_sha256'], before['bridge_sha256']) == (after['server_sha256'], after['bridge_sha256']) == (browser['server_sha256'], browser['bridge_sha256'])
assert b'globalThis.Buffer = undefined' in read('js-join-snapshot-bundle.test.mjs')
assert 'ℹ pass 11' in read('js-join-snapshot-final-connect-tests.log').decode()
lock = json.loads(read('package-lock.json'))
assert lock['packages']['node_modules/cbor-x']['version'] == '1.6.6'
assert b'RESET_BUFFER_MODE' in read('cbor-x-encode.js')
print('Verified three original failures, exact minimal client change, 44 passing final tests, generated-module browser allocation path, pinned native/bridge identities and zero final resource gauges.')
