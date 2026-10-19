"""Offline audit of input capture, FIFO/reentrancy checks and SDK-only timing."""
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
original_hash = hashlib.sha256(read('original-relay.mjs')).hexdigest()
selected_hash = hashlib.sha256(read('selected-relay.mjs')).hexdigest()
assert selected_hash == meta['selected_client_sha256']
assert hashlib.sha256(read('browser-relay.js')).hexdigest() == meta['browser_bundle_sha256']
before = json.loads(read('js-command-snapshot-corrected-before-live-check.json'))
assert not before['passed'] and before['test_exit_code'] == 1
assert before['client_sha256'] == original_hash
assert 'ℹ fail 4' in before['test_output']
for title in ['commands capture nested values before the next microtask sends them',
              'a queued command survives transfer of its original value buffer',
              'a reentrant queued write captures its buffer without corrupting the outer encoder',
              'native variable writes retain the value supplied before command dispatch']:
    assert '✖ ' + title in before['test_output']
prototype = read('js-command-snapshot-eager-corrected-tests.log').decode()
assert '✖ reentrant value getters preserve outer command order and encode once' in prototype
assert 'ℹ fail 1' in prototype
after = json.loads(read('js-command-snapshot-selected-live-check.json'))
assert after['passed'] and after['test_exit_code'] == 0 and after['exit_codes'] == [0, 0]
assert after['client_sha256'] == selected_hash
assert 'ℹ tests 52' in after['test_output'] and 'ℹ pass 52' in after['test_output'] and 'ℹ fail 0' in after['test_output']
assert len(after['final_resource_gauges']) == 6 and all(v == 0 for v in after['final_resource_gauges'].values())
for title in ['reentrant value getters preserve outer command order and encode once',
              'encoding failure retains its queue position and allows the next command',
              'a command invoked from an application-value getter waits for that encoding',
              'a closed client rejects commands without evaluating their values']:
    assert '✔ ' + title in after['test_output']
browser = json.loads(read('js-command-snapshot-bundle-live-check.json'))
assert browser['passed'] and browser['test_exit_code'] == 0 and browser['exit_codes'] == [0, 0]
assert browser['bundle_sha256'] == meta['browser_bundle_sha256']
assert 'ℹ pass 1' in browser['test_output']
assert all(v == 0 for v in browser['final_resource_gauges'].values())
assert b'globalThis.Buffer = undefined' in read('js-command-snapshot-bundle.test.mjs')
assert (before['server_sha256'], before['bridge_sha256']) == (after['server_sha256'], after['bridge_sha256']) == (browser['server_sha256'], browser['bridge_sha256'])
rows = json.loads(read('js-command-snapshot-final-send-screen.json'))
assert [r['variant'] for r in rows] == ['before', 'after', 'after', 'before']
for row in rows:
    assert row['sourceHash'] == (original_hash if row['variant'] == 'before' else selected_hash)
    assert row['iterations'] == 100000 and row['warmup'] == 20000 and row['payloadBytes'] == 1024
    assert row['encodedBytes'] == 331840000 and len(row['nsPerSend']) == 3
    assert all(0 < t < 10000 for t in row['nsPerSend'])
    assert row['command'][:3] == ['taskset', '-c', '8']
print('Verified four original capture failures, rejected reentrant eager prototype, 52 final passes, browser-artifact native acceptance, shared server/bridge identities and source-pinned SDK timing.')
