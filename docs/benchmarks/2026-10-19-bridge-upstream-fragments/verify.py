import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
sources = {}
for name in ['original', 'selected']:
    source = gzip.decompress((root / f'{name}-legacy_bridge.rs.gz').read_bytes())
    assert hashlib.sha256(source).hexdigest() == meta[f'{name}_source_sha256']
    sources[name] = source.decode()
assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
# The sole non-test change enables the existing upstream quota.
original_runtime = sources['original'].split('#[cfg(test)]', 1)[0]
selected_runtime = sources['selected'].split('#[cfg(test)]', 1)[0]
assert selected_runtime.replace('    ' + meta['runtime_patch'] + '\n', '', 1) == original_runtime
before = (root / 'before.log').read_text()
assert 'FAILED' in before and 'upstream welcome parsed the fragment train without yielding' in before
assert 'fragment_poll_counts welcome=3 forwarding=3' in (root / 'after-final.log').read_text()
assert '113 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished `dev` profile' in (root / 'clippy-final.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 61' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
release = json.loads((root / 'release.json').read_text())
assert release['passed'] and release['exit_code'] == 0
assert release['binary_sha256'] == build['binary_sha256']
assert release['baseline_fds'] == release['final_fds'] == 10
assert len(release['cases']) == 8
for welcomed in [False, True]:
    cases = [x for x in release['cases'] if x['welcomed'] == welcomed]
    assert [x['close_code'] for x in cases] == [1002, 1007, 1002, 1009]
    assert all(x['eof'] and x['fragmented_data_verified'] == welcomed for x in cases)
    assert all(x['frames_per_fragmented_message'] == (513 if welcomed else 0) for x in cases)
assert hashlib.sha256((root / 'release-probe.py').read_bytes()).hexdigest() == meta['release_probe_sha256']
print('PASS: one-line runtime fix; original yield failure; three-poll welcome/data; library/JS tests; release FIFO and retirement')
