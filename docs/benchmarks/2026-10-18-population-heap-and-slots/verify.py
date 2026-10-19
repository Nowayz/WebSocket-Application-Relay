#!/usr/bin/env python3
"""Audit allocator evidence and selected slot-accounting checks without traffic."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent
FIELDS = ['live_objects', 'live_bytes', 'allocated_objects', 'allocated_bytes']
PATTERN = re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@\s*(.*)')


def read(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())


def checked_population(record):
    assert record['passed'] and record['correctness_passed']
    assert record['exit_codes'] == [0, 0, 0]
    assert record['fanout_windows'] == {'completed': 32, 'offered': 32}
    assert record['fanout_offered_load_met']
    assert record['args']['users'] == 100000 and record['args']['readers'] == 8192
    assert record['args']['seconds'] == 2
    assert record['args']['churn_workers'] == record['args']['inspectors'] == record['args']['pairs'] == 0
    for key in ['war_connections', 'war_queued_bytes', 'war_variable_bytes']:
        assert record['final_metrics'][key] == 0
    assert record['final_metrics']['war_channels'] == 1
    left = record['events'][-1]['counts']
    right = record['sidecar_events'][-1]['details']['counts']
    pairs = [('war_deliveries_total', 'deliveries'), ('war_disconnected_total', 'normal_closes')]
    if record['args']['kind'] == 'cpp':
        pairs += [('war_sent_messages_total', 'sent_messages')]
    else:
        pairs += [('war_accepted_total', 'joins'), ('war_joined_total', 'joins'),
                  ('war_incoming_messages_total', 'sent_messages'), ('war_incoming_bytes_total', 'sent_bytes'),
                  ('war_delivered_bytes_total', 'delivered_bytes'), ('war_routing_reused_bytes_total', 'value_bytes')]
        for key in ['war_sessions', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
            assert record['final_metrics'][key] == 0
    for metric, field in pairs:
        assert record['final_metrics'][metric] == left[field] + right[field]


def main():
    for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    analysis = json.loads((ROOT / 'current-population-heap-analysis.json').read_text())
    clients = set()
    for kind in ['rust', 'cpp']:
        receipt_bytes = read('current-population-heap-' + kind + '.json')
        receipt = json.loads(receipt_bytes)
        checked_population(receipt)
        assert receipt['profile_diagnostic']
        profile = receipt['heap_profiler']
        assert len(profile['captures']) == 2
        assert profile['driver_sha256'] == hashlib.sha256((ROOT / 'current-population-heap.py').read_bytes()).hexdigest()
        assert profile['dump_helper_sha256'] == hashlib.sha256((ROOT / 'inspection.py').read_bytes()).hexdigest()
        build = json.loads((ROOT / (kind + '-build.json')).read_text())
        assert receipt['sha256']['server'] == build['binary_sha256']
        clients.add((receipt['sha256']['capacity_client'], receipt['sha256']['sidecar']))
        case = analysis['cases'][kind]
        assert case['binary_sha256'] == build['binary_sha256']
        assert case['receipt_sha256'] == hashlib.sha256(receipt_bytes).hexdigest()
        for i, phase in enumerate(['baseline', 'population_ready']):
            data = read(kind + '-' + phase + '.heap')
            descriptor = profile['captures'][i]
            capture = case['captures'][i]
            assert descriptor['phase'] == capture['phase'] == phase
            assert hashlib.sha256(data).hexdigest() == descriptor['sha256'] == capture['heap_sha256']
            assert profile['library'] in data.decode()
            lines = data.decode().splitlines()
            header = PATTERN.fullmatch(lines[0].removeprefix('heap profile:'))
            assert header is not None
            totals = dict(zip(FIELDS, [int(header[i]) for i in range(1, 5)]))
            rows = [PATTERN.fullmatch(line) for line in lines[1:]]
            rows = [row for row in rows if row is not None]
            assert totals == {k: sum(int(row[i + 1]) for row in rows) for i, k in enumerate(FIELDS)}
            assert totals == capture['totals']
            assert totals == {k: sum(row[k] for row in capture['rows']) for k in FIELDS}
            assert totals == {k: sum(group[k] for group in capture['groups'].values()) for k in FIELDS}
            assert [list(map(int, row.groups()[:4])) for row in rows] == [
                [row[k] for k in FIELDS] for row in capture['rows']]
    selected_build = json.loads((ROOT / 'selected-build.json').read_text())
    assert selected_build['passed'] and selected_build['cargo_artifact']['fresh'] is False
    for source in ['src/server.rs', 'src/metrics.rs', 'src/server/arena_tests.rs']:
        assert hashlib.sha256(read('selected-' + Path(source).name)).hexdigest() == selected_build['source_sha256'][source]
    original_server = read('original-server.rs')
    selected_server = read('selected-server.rs')
    assert selected_server[selected_server.index(b'struct UpgradePolicy'):] == original_server[original_server.index(b'struct UpgradePolicy'):]
    record = json.loads(read('connection-slots-100k.json'))
    checked_population(record)
    assert record['sha256']['server'] == selected_build['binary_sha256']
    assert record['ready_metrics']['war_native_connection_slots'] == 131072
    assert record['ready_metrics']['war_sessions'] == 108194
    assert record['final_metrics']['war_native_connection_slots'] == 0
    assert record['final_metrics']['war_routing_copied_bytes_total'] == 0
    clients.add((record['sha256']['capacity_client'], record['sha256']['sidecar']))
    assert len(clients) == 1
    for suffix, count, samples, key in [('.csv', 100000, 8, 'csv_sha256'),
                                        ('-fanout.csv', 8192, 128, 'fanout_csv_sha256')]:
        raw = read('connection-slots-100k' + suffix)
        assert hashlib.sha256(raw).hexdigest() == record[key]
        users = list(csv.DictReader(io.StringIO(raw.decode())))
        assert len(users) == count and len({row['id'] for row in users}) == count
        assert all(int(row['samples']) == samples for row in users)
    for i, variant in enumerate(['control', 'selected']):
        result = json.loads(read(f'connection-slots-churn-{i}-{variant}.json'))
        case = result['cases'][0]
        assert case['passed'] and case['counts_match'] and case['cleaned_up']
        assert case['exit_code'] == case['server_exit_code'] == 0 and not case['server_forced_kill']
        assert case['duration_seconds'] == 1
    assert b'97 passed; 0 failed' in (ROOT / 'connection-slots-tests.log').read_bytes()
    assert b'Finished' in (ROOT / 'connection-slots-clippy.log').read_bytes()
    layout = (ROOT / 'current-population-heap-layout-release.log').read_text()
    assert 'connection=520 stage=320 upgrade=288 websocket=320 socket=184 socket_state=96' in layout
    assert 'peer=80 budget=32 envelope=56' in layout and '2 passed; 0 failed' in layout
    print('PASS: four exact heap captures, executable identities, unchanged connection path, slot gauge and complete selected population observations')


if __name__ == '__main__':
    main()
