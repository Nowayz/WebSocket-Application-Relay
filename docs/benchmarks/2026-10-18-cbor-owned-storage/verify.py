#!/usr/bin/env python3
"""Verify the archived short-run receipts, complete populations and source hashes."""
import csv, hashlib, io, json, lzma
from pathlib import Path
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
def digest(data):
    return hashlib.sha256(data).hexdigest()
def read(path):
    return path.read_bytes()
manifest = json.loads((HERE / 'manifest.json').read_text())
for name, expected in manifest.items():
    assert digest(read(HERE / name)) == expected, name
dep = json.loads((HERE / 'dependency.json').read_text())
assert digest(read(ROOT / dep['path'])) == dep['sha256']
count = 0
for path in sorted((HERE / 'artifacts').glob('*.json')):
    case = json.loads(path.read_text())
    if not isinstance(case, dict) or 'csv_sha256' not in case:
        continue
    assert case['passed'], path
    assert case['sha256']['server'] == '3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
    raw = lzma.decompress(read(path.with_suffix('.csv.xz')))
    assert digest(raw) == case['csv_sha256']
    rows = list(csv.DictReader(io.StringIO(raw.decode())))
    assert len(rows) == 100000 and len({r['id'] for r in rows}) == 100000
    assert all(int(r['samples']) == 32 for r in rows)
    for column, expected in case['latency'].items():
        values = sorted(float(r[column]) for r in rows)
        actual = {name: values[(len(values)-1)*p//100] for name,p in [('median',50),('p95',95),('p99',99),('max',100)]}
        assert actual == expected, (path, column)
    if 'fanout_csv_sha256' in case:
        raw = lzma.decompress(read(path.with_name(path.stem + '-fanout.csv.xz')))
        assert digest(raw) == case['fanout_csv_sha256']
        fanout = list(csv.DictReader(io.StringIO(raw.decode())))
        assert len(fanout) == 8192 and len({r['id'] for r in fanout}) == 8192
        assert all(int(r['samples']) > 0 for r in fanout)
    if not case['args']['profile']:
        count += 1
assert count == 10, count
for path in (HERE / 'artifacts').glob('encode-cost-*.jsonl'):
    for line in path.read_text().splitlines():
        row = json.loads(line)
        assert len(row['ns_per_operation']) == 7
        assert all(v > 0 for v in row['ns_per_operation'])
for name in ['cbor-exact-clean-js-check.json', 'cbor-exact-bridge-adoption.json']:
    assert json.loads((HERE / 'artifacts' / name).read_text())['passed']
print('PASS: archive hashes, dependency pin, ten complete 100k populations, profile population, quantiles, fanout CSVs, constructor batches and bridge receipts')
