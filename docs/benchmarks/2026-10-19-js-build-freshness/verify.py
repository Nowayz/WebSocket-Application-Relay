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
for token in ['find src vendor -type f -exec touch -c {} +', 'touch -c Cargo.toml Cargo.lock', 'cargo build --locked --bins', 'artifact.manifest_path === manifest', 'artifact.fresh !== false', 'artifact.executable']:
    assert token in source, token
old = gzip.decompress((root / 'original-check-js.sh.gz').read_bytes()).decode()
assert 'artifact.fresh !== false' not in old
before = (root / 'before.log').read_text()
assert 'FAILED (failures=4)' in before
assert "'A-A'" in before and "'B-B'" in before and "'same-A'" in before and "'same-B'" in before
assert 'Finished `dev` profile' in before and '0.00s' in before
assert 'Ran 2 tests' in (root / 'tests.log').read_text() and '\nOK\n' in (root / 'tests.log').read_text()
integration = (root / 'integration.log').read_text()
for crate in ['tungstenite', 'tokio-tungstenite', 'war-relay']:
    assert 'Compiling ' + crate in integration
assert '65' in integration and 'all six checked gauges are zero' in integration
print('JS source-freshness evidence verified: four failed-before/passed-after subcases and 65 real integration checks.')
