#!/usr/bin/env python3
"""Offline verification of source associations and both full-population receipts."""
import csv
import hashlib
import io
import json
import lzma
from pathlib import Path
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
dep = load('dependency.json')
assert digest((ROOT / dep['path']).read_bytes()) == dep['sha256']
before = read('sources/before-release-capacity.py').decode()
assert read('sources/capacity.py').decode() == before.replace(
    '                census = established(port, server_inodes)\n',
    '                census = established(port, server_inodes)\n                del server_inodes  # Release gate-only storage before traffic resumes.\n', 1)
for prefix, source in [('owned-census-100k', 'sources/before-release-capacity.py'), ('owned-census-final-100k', 'sources/capacity.py')]:
    case = load('results/' + prefix + '.json')
    assert case['passed'] and case['client_exit_code'] == case['server_exit_code'] == 0
    assert not case.get('forced_kill_pids')
    assert case['server_sha256'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
    assert case['client_sha256'] == 'c74a950efc932b5c7dfe8346e5ebfaf4d3d82a80b68d5b6a0e5bd4c9e655e906'
    assert case['runner_sha256'] == digest(read(source))
    for script, expected in case['runner_sources_sha256'].items():
        path = source if script == 'scripts/capacity.py' else 'sources/' + Path(script).name
        assert digest(read(path)) == expected, script
    assert [e['event'] for e in case['events']] == ['connected', 'ready', 'measured', 'postcheck', 'done']
    for event in case['events']:
        expected = 0 if event['event'] == 'done' else 100000
        census = event['tcp_census']
        assert event['established_server_sockets'] == census['owned_established'] == expected
        assert census['raw_matching_rows'] == census['unique_matching_inodes'] == expected
        assert census['foreign_matching_inodes'] == census['duplicate_rows'] == 0
        assert all(event['metrics'][key] == value for key,value in event['exact_counter_checks'].items())
        assert 0 <= event['metrics']['war_read_buffer_reserved_bytes'] <= event['read_allowance_bound']
    assert case['measurement']['application_deliveries'] == case['measurement']['round_trip']['samples'] == 800000
    rows = list(csv.DictReader(io.StringIO(lzma.decompress(read('results/' + prefix + '.csv.xz')).decode())))
    assert len(rows) == 100000 and len({r['id'] for r in rows}) == 100000
    assert all(int(r['samples']) == 8 for r in rows)
    for row in rows:
        assert 0 <= float(row['p50_ms']) <= float(row['p95_ms']) <= float(row['p99_ms']) == float(row['max_ms'])
        assert float(row['schedule_max_ms']) >= float(row['max_ms'])
    windows = load('results/' + prefix + '.windows.json')['windows']
    assert len(windows) == 2
    assert all(w[name]['samples'] == 400000 for w in windows for name in ['send_to_receive', 'intended_to_receive', 'pre_send_delay'])
print('PASS: archive/dependency/source/executable pins; two 100k populations/1.6M timed probes; all counter, ownership, retirement, unique-ID and sample-count checks')
