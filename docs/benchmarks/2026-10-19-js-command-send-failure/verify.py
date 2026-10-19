#!/usr/bin/env python3
"""Audit command-correlation fault evidence without launching services."""
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
change = b'''      } catch (error) {
        this.#pending.reject(error);
        // A forwarded command may still reply; do not reuse its response slot.
        if (this.ready) this.ws.close(CLIENT_PROTOCOL_ERROR, 'Relay Command Send Failed');
      }'''
assert selected.count(change) == 1
assert selected.replace(change, b'      } catch (error) { this.#pending.reject(error); }') == original
build = json.loads((ROOT / 'build.json').read_text())
assert hashlib.sha256(selected).hexdigest() == build['source_sha256']
assert hashlib.sha256(read('browser-relay.js')).hexdigest() == build['bundle_sha256']
before = json.loads((ROOT / 'js-command-send-before-live-check.json').read_text())
assert before['client_sha256'] == hashlib.sha256(original).hexdigest()
assert not before['passed'] and before['test_exit_code'] != 0 and before['exit_codes'] == [0, 0]
assert 'late false reply resolved the next authentication' in before['test_output']
assert "'fulfilled'" in before['test_output']
proof = json.loads((ROOT / 'js-command-send-original-proof-live-check.json').read_text())
assert proof['passed'] and proof['test_exit_code'] == 0 and proof['exit_codes'] == [0, 0]
assert proof['client_sha256'] == hashlib.sha256(original).hexdigest()
observations = [json.loads(line) for line in proof['test_output'].splitlines() if line.startswith('{')]
assert observations == [{'event': 'original_reply_misassignment', 'original_error_preserved': True,
                         'queued_command': {'status': 'fulfilled', 'value': False}, 'command_frames': 2}]
assert 'fail 1' in (ROOT / 'js-command-send-before-mock.log').read_text()
assert 'pass 24' in (ROOT / 'js-command-send-final-mock.log').read_text()
for stem, count in [('js-command-send-final-live-check', 61), ('js-command-send-bundle-live-check', 5)]:
    record = json.loads((ROOT / (stem + '.json')).read_text())
    assert record['passed'] and record['test_exit_code'] == 0 and record['exit_codes'] == [0, 0]
    assert record['client_sha256'] == build['source_sha256']
    assert f'pass {count}' in record['test_output'] and 'fail 0' in record['test_output']
    assert all(value == 0 for value in record['final_resource_gauges'].values())
    assert record['server_sha256'] == before['server_sha256'] == proof['server_sha256']
    assert record['bridge_sha256'] == before['bridge_sha256'] == proof['bridge_sha256']
    if 'bundle' in stem:
        assert record['bundle_sha256'] == build['bundle_sha256']
print('PASS: exact bad reply/frame-count proof, isolated error-path change, 61 source and five bundle checks with clean shutdown')
