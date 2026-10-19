import hashlib
import json
import statistics
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert meta['selected'] is False
for name, digest in meta['source_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest
control = (root / 'control-timers.rs').read_text()
candidate = (root / 'candidate-timers.rs').read_text()
assert 'self.remove(slot);' in control
assert 'self.heap[index].0 = deadline;' in candidate
assert control[control.index('#[cfg(test)]'):] == candidate[candidate.index('#[cfg(test)]'):]
for kind in ['control', 'candidate']:
    assert '2 passed; 0 failed' in (root / f'{kind}-tests.log').read_text()
initial = json.loads((root / 'unpinned.json').read_text())
pinned = json.loads((root / 'pinned.json').read_text())
assert len(initial) == 36 and len(pinned['runs']) == 18
for population in [1000, 100000]:
    for percent in [0, 1, 100]:
        for kind in ['control', 'candidate']:
            samples = [x for x in initial if x['population'] == population and x['changed_percent'] == percent and x['kind'] == kind]
            assert len(samples) == 3 and all(x['nanoseconds'] > 0 for x in samples)
for percent in [0, 1, 100]:
    medians = {}
    for kind in ['control', 'candidate']:
        samples = [x for x in pinned['runs'] if x['changed_percent'] == percent and x['kind'] == kind]
        assert len(samples) == 3
        assert all(x['population'] == 100000 and x['nanoseconds'] > 0 for x in samples)
        assert len({x['binary_sha256'] for x in samples}) == 1
        medians[kind] = statistics.median(x['nanoseconds'] for x in samples)
    print(f'{percent}% changes: candidate/control elapsed = {medians["candidate"] / medians["control"]:.4f}')
print('Saved timer experiment verified; candidate remains unselected.')
