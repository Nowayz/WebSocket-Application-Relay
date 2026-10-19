import gzip
import hashlib
import json
import re
import statistics
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
assert hashlib.sha256((root / 'legacy_bridge.rs').read_bytes()).hexdigest() == meta['source_sha256']
builds = {}
for name in ['control', 'rejected-two-pass', 'final']:
    b = json.loads((root / (name + '-build.json')).read_text())
    assert b['passed'] and b['cargo_artifact']['fresh'] is False
    builds[name] = b
assert builds['final']['source_sha256']['src/legacy_bridge.rs'] == meta['source_sha256']
assert builds['final']['source_sha256']['examples/bridge_channel_reply_cost.rs'] == hashlib.sha256((root / 'bridge_channel_reply_cost.rs').read_bytes()).hexdigest()
for name, source in [('control', 'original'), ('rejected-two-pass', 'rejected-two-pass')]:
    assert builds[name]['source_sha256']['src/legacy_bridge.rs'] == hashlib.sha256(gzip.decompress((root / (source + '-legacy_bridge.rs.gz')).read_bytes())).hexdigest()
source = (root / 'legacy_bridge.rs').read_text()
assert '.min(count as usize * 256)' in source
assert 'reply.copy_within(populations_start.., name_offset);' in source
assert '120 passed; 0 failed' in (root / 'tests.log').read_text()
assert '65' in (root / 'js.log').read_text() and 'all six checked gauges are zero' in (root / 'js.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
for name in ['comparison', 'repeat', 'rejected-two-pass']:
    data = json.loads((root / (name + '.json')).read_text())
    assert data['passed'] and len(data['ordinary']) == 30
    candidate = 'selected' if name == 'rejected-two-pass' else 'final'
    for channels in [0, 1, 64, 1024, 4096]:
        med = {}
        for kind in ['control', candidate]:
            rows = [r for r in data['ordinary'] if r['channels'] == channels and r['kind'] == kind]
            assert len(rows) == 3
            manifest = builds['control' if kind == 'control' else ('rejected-two-pass' if kind == 'selected' else 'final')]
            assert all(r['binary_sha256'] == manifest['binary_sha256'] and r['nanoseconds'] > 0 for r in rows)
            med[kind] = statistics.median(r['nanoseconds'] / r['iterations'] for r in rows)
        print(f'{name}, {channels} channels: elapsed ratio {med[candidate] / med["control"]:.4f}')
    for row in data['allocation_diagnostics']:
        p = root / (Path(row['heap_path']).name + '.gz')
        raw = gzip.decompress(p.read_bytes())
        assert hashlib.sha256(raw).hexdigest() == row['heap_sha256']
        header = raw.decode().splitlines()[0]
        assert header == row['header']
        m = re.search(r'\[\s*(\d+):\s*(\d+)\]', header)
        assert int(m[1]) == row['total_objects'] and int(m[2]) == row['total_bytes']
old = json.loads((root / 'rejected-two-pass.json').read_text())
new = json.loads((root / 'comparison.json').read_text())
for kind, data, expected in [('control', old, 27), ('final', new, 2)]:
    rows = {r['iterations']: r for r in data['allocation_diagnostics'] if r['kind'] == kind}
    assert (rows[1000]['total_objects'] - rows[0]['total_objects']) / 1000 == expected
    print(f'{kind}: {expected} allocation events/conversion, {(rows[1000]["total_bytes"] - rows[0]["total_bytes"])/1000:.3f} cumulative requested bytes/conversion')
print('Selected bridge channel conversion evidence verified.')
