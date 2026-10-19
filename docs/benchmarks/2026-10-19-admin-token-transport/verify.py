import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for file, digest in meta['selected_source_sha256'].items():
    assert hashlib.sha256((root / Path(file).name).read_bytes()).hexdigest() == digest
    assert build['source_sha256'][file] == digest
assert '2 failed' in (root / 'before-lib.log').read_text()
assert '1 failed' in (root / 'before-cli.log').read_text()
tests = (root / 'tests.log').read_text()
assert '119 passed; 0 failed' in tests and '3 passed; 0 failed' in tests
assert '65' in (root / 'js.log').read_text()
assert 'all six checked gauges are zero' in (root / 'js.log').read_text()
receipt = json.loads((root / 'release.json').read_text())
assert receipt['passed'] and len(receipt['cases']) == 14
assert len({(c['kind'], c['token_case']) for c in receipt['cases']}) == 14
for case in receipt['cases']:
    assert case['passed']
    if case['kind'] == 'selected' and not case['representable']:
        assert case['exit_code'] == 1
        assert 'WAR_ADMIN_TOKEN' in case['diagnostic']
        assert 'redaction-fixture' not in case['diagnostic']
    else:
        assert case['exit_code'] == 0
        assert case['unauthenticated_metrics_status'] == 401
        assert case['authenticated_metrics_status'] == (200 if case['representable'] else 401)
print('Admin token transport evidence verified: 14 release cases and source/test provenance.')
