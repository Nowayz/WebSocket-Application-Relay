#!/usr/bin/env python3
"""Verify immutable evidence and replay population/stage checks without networking."""
import csv
import hashlib
import importlib.util
import json
import lzma
from pathlib import Path
import tempfile
import sys
sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
def digest(data):
    return hashlib.sha256(data).hexdigest()

for name, expected in json.loads((HERE / 'manifest.json').read_text()).items():
    assert digest((HERE / name).read_bytes()) == expected, name
for dep in json.loads((HERE / 'dependencies.json').read_text()):
    assert digest((ROOT / dep['path']).read_bytes()) == dep['sha256'], dep
build = json.loads((HERE / 'artifacts/candidates/latency-stages-client/build.json').read_text())
binary = lzma.decompress((HERE / 'artifacts/candidates/latency-stages-client/capacity.xz').read_bytes())
assert digest(binary) == build['binary_sha256']
assert digest((HERE / 'sources/examples/capacity.rs').read_bytes()) == build['source_sha256']
spec = importlib.util.spec_from_file_location('stage_checks', HERE / 'artifacts/latency-stages-validate.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for label in ['on-1', 'off-1', 'on-2']:
    name = 'latency-stages-' + label
    case = json.loads((HERE / 'artifacts' / (name + '.json')).read_text())
    assert case['sha256']['capacity_client'] == digest(binary)
    assert case['sha256']['sidecar'] == 'ad34e266cd2fb6cb69f270caff448764bdb4f173953ea1d4ac723043ee730de0'
    assert case['sha256']['server'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
    runner = 'artifacts/latency-stages-runner-on-1.py' if label == 'on-1' else 'sources/mixed-capacity.py'
    assert digest((HERE / runner).read_bytes()) == case['sha256']['runner']
    assert all(case['final_metrics'][key] == 0 for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total'])
    with tempfile.TemporaryDirectory() as directory:
        prefix = Path(directory) / name
        for suffix in ['.json', '.windows.json', '.csv', '-fanout.csv']:
            source = HERE / 'artifacts' / (name + suffix)
            data = lzma.decompress(Path(str(source) + '.xz').read_bytes()) if suffix.endswith('.csv') else source.read_bytes()
            Path(str(prefix) + suffix).write_bytes(data)
        module.verify(prefix)
        with prefix.with_suffix('.csv').open() as source:
            rows = list(csv.DictReader(source))
        for column, expected in case['latency'].items():
            values = sorted(float(row[column]) for row in rows)
            assert expected == {key: values[(len(values)-1)*p//100] for key,p in [('median',50),('p95',95),('p99',99),('max',100)]}
        if label != 'off-1':
            assert digest(prefix.with_suffix('.windows.json').read_bytes()) == case.get('latency_stages_windows_sha256', digest(prefix.with_suffix('.windows.json').read_bytes()))
timer = json.loads((HERE / 'artifacts/latency-stages-timer.json').read_text())
assert timer['samples'] == 100000
assert 0 <= timer['p50_ms'] <= timer['p95_ms'] <= timer['p99_ms'] <= timer['max_ms']
print('PASS: archive/dependency/source/binary pins, three complete 100k populations, 9.6M private probes, exact user quantiles, fanout totals, stage conservation/worst-probe identity and timer receipt')
