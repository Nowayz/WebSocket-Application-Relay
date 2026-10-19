import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for name in ['original', 'selected']:
    data = gzip.decompress((root / f'{name}-legacy_bridge.rs.gz').read_bytes())
    assert hashlib.sha256(data).hexdigest() == meta[f'{name}_source_sha256']
assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
before = (root / 'before.log').read_text()
assert 'FAILED' in before and 'ResetWithoutClosingHandshake' in before
text = (root / 'tests.log').read_text()
assert '114 passed; 0 failed' in text
assert 'unavailable_upstream_closes_cleanly_and_single_slot_recovers ... ok' in text
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 61' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
release = json.loads((root / 'release.json').read_text())
assert release['passed'] and release['exit_code'] == 0
assert release['binary_sha256'] == build['binary_sha256']
assert release['baseline_fds'] == release['final_fds'] == 10
assert [x['case'] for x in release['cases']] == ['refused-0', 'refused-1', 'stalled-upgrade']
assert all(x['close_code'] == 1013 and x['reason'] == 'Upstream Unavailable' and x['eof'] for x in release['cases'])
assert 9.5 <= release['cases'][-1]['elapsed_seconds'] <= 12
assert hashlib.sha256((root / 'release-probe.py').read_bytes()).hexdigest() == meta['release_probe_sha256']
print('PASS: original unclean failure; selected source/build; recovery regression; 114 library and 61 JS tests; release refusal/deadline/retirement')
