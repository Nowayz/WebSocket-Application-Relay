#!/usr/bin/env python3
"""Verify retained successes, expected failure and native candidate attribution."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sha = lambda data: hashlib.sha256(data).hexdigest()
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert sha((ROOT / name).read_bytes()) == expected, name
builds = json.loads((ROOT / 'builds.json').read_text())
assert builds['native_candidate_selected'] is False
for path in sorted((ROOT / 'raw').glob('*.json.gz')):
    x = json.loads(gzip.decompress(path.read_bytes()))
    if path.name == 'socket-header-profile.json.gz':
        assert not x['passed'] and 'outer deadline' in x['error']
        assert x['runner_sha256'] == sha((ROOT / 'raw/socket-header-initial-inspection.py').read_bytes())
        print(path.name, 'expected cleanup failure retained')
        continue
    assert x['passed'] and x['client_exit_code'] == x['server_exit_code'] == 0
    assert x['runner_sha256'] == sha((ROOT / 'raw/inspection.py').read_bytes())
    assert x['server_sha256'] in builds['binaries'].values()
    final = x['events'][-1]
    assert final['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total']:
        assert final['metrics'][key] == 0, (path.name, key)
    assert final['metrics']['war_channels'] == 1
    if path.name == 'socket-header-default-close-check.json.gz':
        assert x['args']['fanout_close_window'] == 1 and '--fanout-close-window' not in x['command']
    else:
        assert x['args']['fanout_close_window'] == 32
        assert x['command'][x['command'].index('--fanout-close-window') + 1] == '32'
    print(path.name, 'verified')
print('Archive hashes, expected failure, binary/runner identities and retirement verified.')
