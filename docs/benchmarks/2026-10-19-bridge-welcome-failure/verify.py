import gzip,hashlib,json
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
before=(r/'before.log').read_text();assert '0 passed; 2 failed' in before and before.count('ResetWithoutClosingHandshake')==2
selected=(r/'tests.log').read_text();assert '130 passed; 0 failed' in selected
assert 'welcome_failure stalled=false' in selected and 'welcome_failure stalled=true' in selected
integration=(r/'integration.log').read_text();assert 'ℹ pass 65' in integration and 'all six checked gauges are zero' in integration
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
build=json.loads((r/'build.json').read_text());assert build['passed'] and build['cargo_artifact']['fresh'] is False
source=gzip.decompress((r/'selected-legacy-bridge.rs.gz').read_bytes())
assert hashlib.sha256(source).hexdigest()==build['source_sha256']['src/legacy_bridge.rs']
old=gzip.decompress((r/'before-legacy-bridge.rs.gz').read_bytes()).decode()
new=source.decode()
assert old[old.index('#[cfg(test)]'):]==new[new.index('#[cfg(test)]'):]
assert 'upstream.send(Message::Binary(subscription)).await?;\n    let welcome = timeout' in old
assert 'let welcome = timeout(WAIT, async {\n        upstream.send(Message::Binary(subscription)).await?;' in new
assert '.context("upstream welcome timeout")??;' in old and '.context("upstream welcome timeout")??;' not in new
print('Welcome-failure evidence verified: both failed-before cases, 130 passing tests, 65 JS checks and fresh release build.')
