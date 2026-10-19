import gzip,hashlib,json
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
assert '0 passed; 2 failed' in (r/'before.log').read_text()
assert '2 passed; 0 failed' in (r/'focused.log').read_text()
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
mutant=(r/'mutant.log').read_text();assert '0 passed; 1 failed' in mutant and 'second-' in mutant and 'first-' in mutant
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
integration=(r/'integration.log').read_text();assert 'ℹ pass 65' in integration and 'all six checked gauges are zero' in integration
source=gzip.decompress((r/'selected-legacy-bridge.rs.gz').read_bytes())
build=json.loads((r/'build.json').read_text());assert build['passed'] and build['cargo_artifact']['fresh'] is False
assert hashlib.sha256(source).hexdigest()==build['source_sha256']['src/legacy_bridge.rs']
text=source.decode();assert 'incoming = legacy.next(), if pending.is_none()' in text
assert text.count('Some(message) => Some(Ok(message)),')==1
expected=text.replace('Some(message) => Some(Ok(message)),','Some(_) => legacy.next().await,')
assert expected==gzip.decompress((r/'mutant-legacy-bridge.rs.gz').read_bytes()).decode()
print('Frontend welcome handling verified: failed-before retirement, 133 passing tests, FIFO mutation sensitivity, 65 JS checks and fresh release build.')
