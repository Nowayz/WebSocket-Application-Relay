import hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for file, digest in meta['selected_source_sha256'].items():
    assert hashlib.sha256((root / Path(file).name).read_bytes()).hexdigest() == digest
    assert build['source_sha256'][file] == digest
library = (root / 'legacy_bridge.rs').read_text()
cli = (root / 'war-legacy-bridge.rs').read_text()
assert 'pub fn validate_connection_limits(' in library
assert 'validate_connection_limits(max_connections, max_connections_per_ip)?;' in library
assert cli.index('validate_connection_limits(') < cli.index('TcpListener::bind(') < cli.index('legacy bridge listening')
before = (root / 'before.log').read_text()
assert '2 failed' in before and 'invalid_global_connection_limits_are_reported_before_binding' in before
assert 'invalid_per_ip_connection_limit_is_reported_before_binding' in before
text = (root / 'tests.log').read_text()
assert '117 passed; 0 failed' in text and '3 passed; 0 failed' in text
assert 'listen_backlog_is_positive_and_bounded ... ok' in text
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 65' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
p = json.loads((root / 'release.json').read_text())
assert p['passed'] and len(p['cases']) == 8
for case in p['cases']:
    assert case['passed'] and case['exit_code'] == 1
    if case['kind'] == 'selected':
        assert case['binary_sha256'] == build['binary_sha256']
    if case['kind'] == 'selected' and case['invalid']:
        assert 'cannot bind legacy bridge' not in case['diagnostic']
    else:
        assert 'cannot bind legacy bridge' in case['diagnostic']
p = json.loads((root / 'listening.json').read_text())
assert p['passed'] and len(p['cases']) == 2
assert p['cases'][0]['advertised_listening'] and not p['cases'][1]['advertised_listening']
assert all(case['exit_code'] == 1 and 'invalid bridge connection limit' in case['stderr'] for case in p['cases'])
print('PASS: shared prebind validation; original masked errors/log; unchanged valid bounds; 117 library/3 CLI integration/65 JS tests; ten release cases')
