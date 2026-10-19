import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for name in ['original', 'selected']:
    source = gzip.decompress((root / f'{name}-legacy_bridge.rs.gz').read_bytes())
    assert hashlib.sha256(source).hexdigest() == meta[f'{name}_source_sha256']
assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
for name, stage in [('before.log', 'false'), ('before-postwelcome.log', 'true')]:
    text = (root / name).read_text()
    assert 'FAILED' in text and 'ResetWithoutClosingHandshake' in text
    assert f'welcomed={stage}' in text
assert '112 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'upstream_frame_errors_close_legacy_cleanly_before_and_after_welcome ... ok' in (root / 'tests.log').read_text()
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 61' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
failed = json.loads((root / 'release.json').read_text())
assert not failed['passed'] and not failed['cases'] and failed['exit_code'] == 0
release = json.loads((root / 'release-v2.json').read_text())
assert release['passed'] and release['exit_code'] == 0
assert release['binary_sha256'] == build['binary_sha256']
assert release['baseline_fds'] == release['final_fds'] == 10
assert len(release['cases']) == 8
for welcomed in [False, True]:
    cases = [x for x in release['cases'] if x['welcomed'] == welcomed]
    assert [x['close_code'] for x in cases] == [1002, 1007, 1002, 1009]
    assert [x['reason'] for x in cases] == ['Protocol Error', 'Invalid UTF-8', 'Protocol Error', 'Message Too Large']
    assert all(x['eof'] for x in cases)
assert hashlib.sha256((root / 'release-probe.py').read_bytes()).hexdigest() == meta['release_probe_sha256']
print('PASS: both original failures; selected source/build; 112 library and 61 JS tests; eight release faults and descriptor retirement')
