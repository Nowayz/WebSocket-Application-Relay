"""Offline audit of the isolated deadline-update screen; no load is started."""
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name
meta = json.loads((root/'builds.json').read_text())
assert not meta['selected']
assert meta['timers'] == 100000 and meta['updates_per_case'] == 800000
for name in ['timer-retime-micro.json', 'timer-retime-pinned-micro.json']:
    rows = json.loads((root/'raw'/name).read_text())
    assert [r['variant'] for r in rows] == ['control', 'candidate', 'candidate', 'control']
    for row in rows:
        assert set(row['results']) == {'postpone', 'advance', 'mixed', 'unchanged'}
        assert all(0 < v < 1 for v in row['results'].values())
        if 'pinned' in name:
            assert row['command'][:3] == ['taskset', '-c', '0']
    if 'pinned' in name:
        control = [r['results'] for r in rows if r['variant'] == 'control']
        candidate = [r['results'] for r in rows if r['variant'] == 'candidate']
        assert max(r['advance'] for r in candidate) < min(r['advance'] for r in control)
        assert max(r['postpone'] for r in candidate) > max(r['postpone'] for r in control)
        assert min(r['mixed'] for r in candidate) < min(r['mixed'] for r in control)
        assert max(r['mixed'] for r in candidate) > max(r['mixed'] for r in control)
assert '2 passed; 0 failed' in (root/'raw/timer-retime-candidate-tests.log').read_text()
for variant in ['control', 'candidate']:
    source = (root/'raw'/f'timer-retime-{variant}-bench.rs').read_text()
    assert 'n=100000usize' in source and '1..=8usize' in source
    assert 'assert_eq!(count,n)' in source and 'black_box(slot)' in source
    assert meta['build_commands'][0 if variant == 'control' else 1][:4] == ['rustc', '--edition', '2024', '-O']
print('Verified paired timer sources/harnesses, eight fresh-process result sets, pinned mixed tradeoffs, compiler metadata and candidate reference tests; candidate remains unselected.')
