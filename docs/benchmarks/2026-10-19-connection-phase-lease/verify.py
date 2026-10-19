import argparse
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path
import re

root = Path(__file__).resolve().parent
repo = root.parents[2]
parser = argparse.ArgumentParser()
parser.add_argument('--check-local-csv', action='store_true')
args = parser.parse_args()
load = lambda name: json.loads(gzip.decompress((root / name).read_bytes()))
summary = json.loads((root / 'summary.json').read_text())
builds = {kind: json.loads((root / f'{kind}-build.json').read_text()) for kind in ['control', 'candidate']}
for build in builds.values():
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
for path in (root / 'source').rglob('*.rs.gz'):
    name = str(path.relative_to(root / 'source'))[:-3]
    assert hashlib.sha256(gzip.decompress(path.read_bytes())).hexdigest() == builds['candidate']['source_sha256'][name]
assert 'connection=520 stage=320 upgrade=288 lease=16 phase_lease=320' in (root / 'layout.log').read_text()
assert 'connection=504 stage=320 upgrade=288 lease=16 phase_lease=320' in (root / 'candidate-layout.log').read_text()
assert '114 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 61' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
zero = ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_native_connection_slots', 'war_routing_copied_bytes_total']

def gates(receipt, kind):
    assert receipt['passed'] and receipt['correctness_passed'] and receipt['exit_codes'] == [0, 0, 0]
    assert receipt['sha256']['server'] == builds[kind]['binary_sha256']
    assert receipt['fanout_windows'] == {'completed': 32, 'offered': 32} and receipt['fanout_offered_load_met']
    assert receipt['ready_metrics']['war_native_connection_slots'] == 131072
    for key in zero:
        assert receipt['final_metrics'][key] == 0
    for key, value in receipt['final_metrics'].items():
        if any(word in key for word in ['errors_total', 'rejected_total', 'slow_consumers_total', 'timeouts_total', 'aborted_writes_total', 'close_failures_total', 'transport_disconnects_total', 'global_pressure_total']):
            assert value == 0

for i, kind in enumerate(['control', 'candidate', 'candidate', 'control']):
    p = load(f'{i}-{kind}.json.gz')
    gates(p, kind)
    assert not p.get('profile_diagnostic', False)
    assert p['args']['users'] == 100000 and p['args']['readers'] == 8192 and p['args']['seconds'] == 2
    assert p['latency_sampling']['total_private_observations'] == 800000
    assert p['sidecar_events'][-1]['details']['counts']['joins'] - p['args']['readers'] - 2 == 256
    s = summary[i]
    assert s['cpu_seconds'] == p['server_cpu_seconds'] and s['rss_kib'] == p['before']['rss_kib']
    assert s['private'] == p['latency']['max_ms'] and s['fanout'] == p['fanout_exact_ms']['max_ms']
    if args.check_local_csv:
        prefix = (repo / p['args']['output']).with_suffix('')
        for path, digest, expected, count, samples in [
            (Path(str(prefix) + '.csv'), p['csv_sha256'], p['latency'], 100000, 8),
            (Path(str(prefix) + '-fanout.csv'), p['fanout_csv_sha256'], p['fanout_exact_ms'], 8192, 128),
        ]:
            data = path.read_bytes()
            assert hashlib.sha256(data).hexdigest() == digest
            rows = list(csv.DictReader(io.StringIO(data.decode())))
            assert len(rows) == count and len({row['id'] for row in rows}) == count
            assert all(int(row['samples']) == samples for row in rows)
            for column, reported in expected.items():
                values = sorted(float(row[column]) if column in row else int(row[column.removesuffix('_ms') + '_ns']) / 1e6 for row in rows)
                actual = {name: values[(len(values) - 1) * percentile // 100] for name, percentile in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]}
                assert actual == reported, (path, column)
analysis = load('heap-analysis.json.gz')
heap_totals = {}
pattern = re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@.*')
for kind in ['control', 'candidate']:
    p = load(f'heap-{kind}.json.gz')
    gates(p, kind)
    assert p['profile_diagnostic']
    totals = []
    for i, capture in enumerate(p['heap_profiler']['captures']):
        data = (root / f"{kind}-{capture['phase']}.heap").read_bytes()
        assert hashlib.sha256(data).hexdigest() == capture['sha256']
        lines = data.decode().splitlines()
        header = pattern.fullmatch(lines[0].removeprefix('heap profile:'))
        wanted = tuple(map(int, header.groups()))
        rows = [tuple(map(int, match.groups())) for line in lines[1:] if (match := pattern.fullmatch(line))]
        assert tuple(map(sum, zip(*rows))) == wanted
        stored = analysis['cases'][kind]['captures'][i]['totals']
        assert wanted == tuple(stored[k] for k in ['live_objects', 'live_bytes', 'allocated_objects', 'allocated_bytes'])
        totals.append(wanted)
    heap_totals[kind] = totals
assert heap_totals['control'][0][:2] == heap_totals['candidate'][0][:2] == (306, 204305)
assert heap_totals['control'][1][:2] == (343717, 116427110)
assert heap_totals['candidate'][1][:2] == (343717, 114326630)
assert 116427110 - 114326630 == 2100480 and 131072 * (520 - 504) == 2097152
print('PASS: source/layout, exact heap totals, four ordinary 100k gates and unselected tradeoff' + ('; eight local CSV hashes and all population summaries recomputed' if args.check_local_csv else '; local CSVs not read'))
