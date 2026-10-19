import gzip, hashlib, json, statistics
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
def read(name):
    return gzip.decompress((root / 'raw' / (name + '.gz')).read_bytes()).decode()
for kind in ['control','candidate']:
    build = json.loads(read(kind + '-build.json'))
    assert build['passed'] and not build['cargo_artifact']['fresh']
    assert hashlib.sha256((root / 'raw' / (kind + '-metrics.rs')).read_bytes()).hexdigest() == build['source_sha256']['src/metrics.rs']
    assert hashlib.sha256((root / 'raw/metrics_cost.rs').read_bytes()).hexdigest() == build['source_sha256']['examples/metrics_cost.rs']
a = (root / 'raw/control-metrics.rs').read_bytes()
b = (root / 'raw/candidate-metrics.rs').read_bytes()
assert a.replace(b'String::with_capacity(2500)', b'String::with_capacity(8192)') == b
outputs = [(root / 'raw' / f'render-{i}.prom').read_bytes() for i in range(8)]
assert len(set(outputs)) == 1 and len(outputs[0]) == 7336
values = [line.split()[1] for line in outputs[0].decode().splitlines() if not line.startswith('#')]
assert len(values) == 42 and len(outputs[0]) + sum(20-len(v) for v in values) == 8125 < 8192
rows = json.loads(read('metrics-reserve-results.json'))
assert [r['kind'] for r in rows] == ['control','candidate','candidate','control','candidate','control','control','candidate']
for row in rows:
    assert row['length'] == 7336 and row['capacity'] == (10000 if row['kind'] == 'control' else 8192)
    assert len(row['ns_per_render']) == 7 and all(n > 0 for n in row['ns_per_render'])
for kind in ['control','candidate']:
    medians = [statistics.median(r['ns_per_render']) for r in rows if r['kind'] == kind]
    print(kind, 'process median range ns:', min(medians), max(medians))
assert '91 passed; 0 failed' in read('metrics-reserve-tests.log')
assert 'Finished' in read('metrics-reserve-clippy.log') and 'error:' not in read('metrics-reserve-clippy.log')
print('Byte-identical exposition, capacity reduction, numeric-width bound, source/build hashes, tests and Clippy verified.')
