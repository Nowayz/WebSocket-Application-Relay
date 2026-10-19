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
assert 'setTimeout(fail, 1000)' in source
assert 'SECONDS + 5' in source
assert 'setTimeout(fail' not in gzip.decompress((root / 'original-check-js.sh.gz').read_bytes()).decode()
before = (root / 'before.log').read_text()
assert 'FAILED (failures=1)' in before and 'stalled HTTP request' in before
selected = (root / 'tests.log').read_text()
assert 'Ran 4 tests' in selected and '\nOK\n' in selected
integration = (root / 'integration.log').read_text()
assert 'ℹ pass 65' in integration and 'all six checked gauges are zero' in integration
print('Readiness evidence verified: stalled-response failure before, four passing methods after, 65 integration checks and retired resources.')
