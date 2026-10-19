import gzip, hashlib, json, re
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
sources = {}
for name, digest in meta['source_sha256'].items():
    sources[name] = gzip.decompress((root / f'{name}-admin.rs.gz').read_bytes())
    assert hashlib.sha256(sources[name]).hexdigest() == digest
assert sources['selected'].split(b'#[cfg(test)]', 1)[0] == sources['original'].split(b'#[cfg(test)]', 1)[0]
builds = {kind: json.loads((root / f'{kind}-build.json').read_text()) for kind in ['control', 'candidate']}
for kind, build in builds.items():
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    assert build['source_sha256']['src/admin.rs'] == meta['source_sha256']['original' if kind == 'control' else 'candidate']
zero = ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']
for i, kind in enumerate(['control', 'candidate', 'candidate', 'control', 'candidate', 'control']):
    p = json.loads((root / f'{i}-{kind}.json').read_text())
    assert p['passed'] and p['exit_code'] == 0 and p['requests'] == 50000 and not p['profile_diagnostic']
    assert p['server_sha256'] == builds[kind]['binary_sha256']
    assert all(p['final_gauges'][key] == 0 for key in zero)
summary = json.loads((root / 'allocation-summary.json').read_text())
pattern = re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@\s*(.*)')
for kind, count, size in [('control', 23000, 10220000), ('candidate', 22000, 10218000)]:
    p = json.loads((root / f'{kind}-heap.json').read_text())
    assert p['passed'] and p['exit_code'] == 0 and p['requests'] == 1000 and p['profile_diagnostic']
    assert p['server_sha256'] == builds[kind]['binary_sha256']
    totals = []
    for capture in p['captures']:
        data = (root / f"{kind}-{capture['phase']}.heap").read_bytes()
        assert hashlib.sha256(data).hexdigest() == capture['sha256']
        lines = data.decode().splitlines()
        match = pattern.fullmatch(lines[0].removeprefix('heap profile:'))
        total = tuple(map(int, match.groups()[:4]))
        rows = [tuple(map(int, m.groups()[:4])) for line in lines[1:] if (m := pattern.fullmatch(line))]
        assert tuple(map(sum, zip(*rows))) == total
        totals.append(total)
    assert totals[1][2] - totals[0][2] == summary[kind]['allocated_objects'] == count
    assert totals[1][3] - totals[0][3] == summary[kind]['allocated_bytes'] == size
    assert totals[0][:2] == totals[1][:2] == (232, 131386)
assert len(summary['control']['two_byte_groups']) == 1 and not summary['candidate']['two_byte_groups']
assert '117 passed; 0 failed' in (root / 'selected-tests.log').read_text()
assert 'Finished `dev` profile' in (root / 'selected-clippy.log').read_text()
js = (root / 'selected-js.log').read_text()
assert 'pass 65' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
print('PASS: selected runtime unchanged; six fixed-work cases; exact allocation totals; wire regression/library/JS tests')
