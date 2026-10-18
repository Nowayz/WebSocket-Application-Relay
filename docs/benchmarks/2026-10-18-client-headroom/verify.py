#!/usr/bin/env python3
"""Verify the archived short tests without starting any relay or client."""
import csv
import hashlib
import importlib.util
import io
import json
import lzma
from pathlib import Path
import sys
import tempfile
sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
def digest(data):
    return hashlib.sha256(data).hexdigest()
def read(name):
    return (HERE / name).read_bytes()
def load(name):
    return json.loads(read(name))
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, HERE / path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result
for name, expected in load('manifest.json').items():
    assert digest(read(name)) == expected, name
for name, expected in load('decoded-sha256.json').items():
    assert digest(lzma.decompress(read(name))) == expected, name
for dep in load('dependencies.json'):
    assert digest((ROOT / dep['path']).read_bytes()) == dep['sha256']
builds = {row['label']: row for row in load('artifacts/client-event-build.json')}
for label, metadata in builds.items():
    prefix = 'artifacts/candidates/client-event-' + label
    assert digest(lzma.decompress(read(prefix + '/capacity.xz'))) == metadata['binary_sha256']
    assert digest(read(prefix + '/capacity.rs')) == metadata['source_sha256']
control = read('artifacts/candidates/client-event-control/capacity.rs').decode()
assert read('artifacts/candidates/client-event-8/capacity.rs').decode() == control.replace('.worker_threads(args.client_workers)', '.worker_threads(args.client_workers)\n        .event_interval(8)', 1)
lag = module('lag', 'sources/scripts/mixed-capacity.py')
stages = module('stages', 'sources/latency-stages-validate.py')
summary = load('artifacts/client-headroom-summary.json')
assert len(summary) == 11
summaries = {row['prefix']: row for row in summary}
private_samples = 0
server_hash = '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
topology = {}
for line in load('artifacts/client-event-control-2.json')['cpu_topology'].splitlines():
    if line and not line.startswith('#'):
        cpu, core, socket, node = line.split(',')
        topology[int(cpu)] = (int(socket), int(core))
for campaign_name in ['event', 'workers', 'placement']:
    campaign = load('artifacts/client-' + campaign_name + '-campaign.json')
    assert campaign['passed']
    for run in campaign['runs']:
        assert run['exit_code'] == 0
        prefix = str(Path(run['receipt']).with_suffix(''))
        case = load(prefix + '.json')
        assert case['passed']
        raw = lzma.decompress(read(prefix + '.csv.xz'))
        rows = list(csv.DictReader(io.StringIO(raw.decode())))
        assert len(rows) == 100000 and len({row['id'] for row in rows}) == 100000
        assert all(int(row['samples']) == 32 for row in rows)
        private_samples += sum(int(row['samples']) for row in rows)
        calculated = {}
        for column in ['max_ms', 'schedule_max_ms']:
            values = sorted(float(row[column]) for row in rows)
            calculated[column] = {key: values[(len(values)-1)*q//100] for key,q in [('median',50),('p95',95),('p99',99),('max',100)]}
        assert calculated == summaries[prefix]['user_worst_ms']
        timeline = load(prefix + '.windows.json')
        assert len(timeline['windows']) == 8
        if campaign_name != 'placement':
            assert case['sha256']['server'] == server_hash
            label = run['label'] if campaign_name == 'event' else 'control'
            assert case['sha256']['capacity_client'] == builds[label]['binary_sha256']
            assert case['sha256']['sidecar'] == 'ad34e266cd2fb6cb69f270caff448764bdb4f173953ea1d4ac723043ee730de0'
            runner = 'artifacts/client-workers-4-runner.py' if campaign_name == 'workers' and run['workers'] == 4 else 'artifacts/client-event-runner.py'
            assert digest(read(runner)) == case['sha256']['runner']
            assert case['fanout_windows']['offered'] == 128
            assert 0 < case['fanout_windows']['completed'] <= 128
            result = lag.generator_lag(timeline, 4)
            assert result['exceeded_one_period'] == (campaign_name == 'workers' and run['workers'] == 4)
            with tempfile.TemporaryDirectory() as directory:
                target = Path(directory) / Path(prefix).name
                for suffix in ['.json', '.windows.json', '.csv', '-fanout.csv']:
                    contents = lzma.decompress(read(prefix + suffix + '.xz')) if suffix.endswith('.csv') else read(prefix + suffix)
                    Path(str(target) + suffix).write_bytes(contents)
                stages.verify(target)
        else:
            assert case['server_sha256'] == server_hash and case['client_sha256'] == builds['control']['binary_sha256']
            assert digest(read('artifacts/client-placement-diagnostic-capacity.py')) == case['runner_sha256']
            assert case['client_exit_code'] == case['server_exit_code'] == 0
            assert case['measurement']['application_deliveries'] == 3200000
            for event in case['events']:
                expected = 0 if event['event'] == 'done' else 100000
                census = event['established_scan']
                assert census['rows'] == census['unique_inodes'] == census['owned_unique'] == expected
                assert not census['duplicates'] and not census['foreign']
                assert all(event['metrics'][key] == value for key,value in event['exact_counter_checks'].items())
            client_pins = {pin['cpu'] for pin in case['thread_affinity'] if pin['name'] == 'tokio-rt-worker'}
            server_pins = {pin['cpu'] for pin in case['thread_affinity'] if pin['name'].startswith('war-io-')}
            assert len(client_pins) == 8 and server_pins == set(range(8))
            client_cores = {topology[cpu] for cpu in client_pins}
            assert len(client_cores) == (4 if run['label'] == 'smt' else 8)
            assert not client_cores & {topology[cpu] for cpu in server_pins}
assert private_samples == 35200000
smoke = load('artifacts/client-headroom-smoke.json')
assert smoke['passed'] and all(code == 0 for code in smoke['exit_codes'])
assert digest(read('sources/scripts/mixed-capacity.py')) == smoke['sha256']['runner']
smoke_timeline = read('artifacts/client-headroom-smoke.windows.json')
assert digest(smoke_timeline) == smoke['latency_windows_sha256']
assert smoke['generator_lag'] == lag.generator_lag(json.loads(smoke_timeline), 4)
assert smoke['generator_lag']['available'] and not smoke['generator_lag']['exceeded_one_period']
failed = load('artifacts/client-placement-smt-1.json')
assert not failed['passed'] and all(event['event'] != 'measured' for event in failed['events'])
print('PASS: all pins; eleven 100k populations/35.2M probes; exact user quantiles; seven mixed stage/fanout matrices; four owned socket censuses/physical affinities; overload classification; final integration smoke; failed setup retained')
