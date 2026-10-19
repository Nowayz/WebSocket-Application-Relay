#!/usr/bin/env python3
"""Audit test-only scope and the deliberately rejected singleton shortcut."""
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

original = read('original-fairness-tests.rs')
selected = read('selected-fairness-tests.rs')
assert selected.startswith(original)
assert selected.count(b'#[test]') == original.count(b'#[test]') + 2
assert b'enabled observer missed the singleton broadcast' in selected
assert json.loads((ROOT / 'singleton-snapshot-differences.json').read_text()) == ['src/server/fairness_tests.rs']
snapshot = json.loads((ROOT / 'snapshot.json').read_text())
assert hashlib.sha256(selected).hexdigest() == snapshot['source_sha256']['src/server/fairness_tests.rs']
module = read('server-module.rs')
assert hashlib.sha256(module).hexdigest() == snapshot['source_sha256']['src/server.rs']
assert b'#[cfg(test)]\nmod fairness_tests;' in module
relay = read('original-relay.rs')
assert hashlib.sha256(relay).hexdigest() == snapshot['source_sha256']['src/relay.rs']
mutation = b'''        if target == BROADCAST && peer.channel.as_ref() != GLOBAL_CHANNEL
            && state.channels.get(&peer.channel).is_some_and(|channel| channel.members.len() == 1)
        {
            return Ok(1);
        }
'''
mutant = read('mutant-relay.rs')
assert mutant.count(mutation) == 1 and mutant.replace(mutation, b'') == relay
failure = (ROOT / 'singleton-mutant-tests.log').read_text()
assert 'enabled observer missed the singleton broadcast' in failure
assert 'left: 0' in failure and 'right: 1' in failure
assert 'FAILED. 0 passed; 1 failed' in failure
assert '102 passed; 0 failed' in (ROOT / 'singleton-selected-tests.log').read_text()
assert 'Finished' in (ROOT / 'singleton-clippy.log').read_text()
print('PASS: test-only scope, restored relay, two new contracts, deliberate observer-loss detection and 102 passing tests')
