"""Audit the directory-separation regression without compiling or serving."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected
before = gzip.decompress((ROOT / 'raw/snapshot-overlap-before.log.gz').read_bytes()).decode()
after = gzip.decompress((ROOT / 'raw/snapshot-overlap-after.log.gz').read_bytes()).decode()
assert '0 != 2' in before and '"passed": true' in before and 'failures=1' in before
assert 'Ran 6 tests' in after and 'OK' in after
assert not build['runtime_changed'] and build['new_load_runs'] == 0
print('Checksums, exact source identities, original acceptance failure and six passing real-Cargo tests verified.')
