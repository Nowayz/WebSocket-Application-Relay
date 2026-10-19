import gzip
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert meta['runtime_changed'] is False
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
source = (root / 'check-js.sh').read_text()
assert 'for fixture_var in "${!WAR_@}"; do unset "$fixture_var"; done' in source
assert source.index('for fixture_var in') < source.index('cargo build --locked')
assert 'for fixture_var in' not in gzip.decompress((root / 'original-check-js.sh.gz').read_bytes()).decode()
test = (root / 'test-js-build-freshness.py').read_text()
for token in ['WAR_FUTURE_FIXTURE_SENTINEL', "child['war']", "child['keep']", "child['target']", 'self.assertEqual(env[name], value)']:
    assert token in test
before = (root / 'before.log').read_text()
assert 'FAILED (failures=1)' in before and 'WAR_NO_ADMIN' in before
assert 'Ran 3 tests' in (root / 'tests.log').read_text() and '\nOK\n' in (root / 'tests.log').read_text()
scenario = json.loads((root / 'scenario.json').read_text())
assert scenario['before_failure_observed'] and scenario['after_exit_code'] == 0
assert scenario['before']['WAR_NO_ADMIN'] == scenario['after']['WAR_NO_ADMIN'] == 'true'
assert scenario['after']['WAR_QUEUE_BYTES'] == '1' and scenario['after']['WAR_ALLOWED_ORIGINS'] == 'not-an-origin'
old = (root / 'before-integration.log').read_text()
assert 'Finished `dev` profile' in old and 'ℹ pass 65' not in old
integration = (root / 'integration.log').read_text()
assert '65' in integration and 'all six checked gauges are zero' in integration
print('Fixture environment isolation verified: failed-before regression, passing three-method suite and 65 checks under conflicting settings.')
