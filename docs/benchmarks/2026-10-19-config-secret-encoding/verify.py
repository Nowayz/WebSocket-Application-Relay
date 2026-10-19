import hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for file, digest in meta['selected_source_sha256'].items():
    assert hashlib.sha256((root / Path(file).name).read_bytes()).hexdigest() == digest
    assert build['source_sha256'][file] == digest
config = (root / 'config.rs').read_text()
assert 'pub fn load_secrets(&mut self) -> Result<()>' in config
assert config.index('let admin_token = read("WAR_ADMIN_TOKEN")?;') < config.index('self.auth_password = auth_password;')
assert 'config.load_secrets()?;' in (root / 'main.rs').read_text()
assert 'FAILED' in (root / 'before.log').read_text() and 'must identify the invalid variable' in (root / 'before.log').read_text()
tests = (root / 'tests.log').read_text()
assert '114 passed; 0 failed' in tests and '2 passed; 0 failed' in tests
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 65' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
p = json.loads((root / 'release.json').read_text())
assert p['passed'] and len(p['cases']) == 4
for case in p['cases']:
    assert case['passed'] and 'redaction-fixture' not in case['diagnostic']
    if case['kind'] == 'control':
        assert case['exit_code'] == 0 and case['unauthenticated_metrics_status'] == 200
    else:
        assert case['exit_code'] == 1 and case['binary_sha256'] == build['binary_sha256']
        assert case['variable'] in case['diagnostic'] and 'UTF-8' in case['diagnostic']
        assert 'relay listening' not in case['diagnostic']
print('PASS: fallible source/CLI propagation; original failure; six configuration child cases; four release cases; 114 library and 65 JS tests')
