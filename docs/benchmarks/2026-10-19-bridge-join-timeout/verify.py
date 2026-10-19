"""Audit saved join-timeout evidence without running the relay."""
import gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in json.loads((p/'sha256.json').read_text()).items():assert sha((p/name).read_bytes())==digest,name
build=json.loads((p/'build.json').read_text());assert build['passed'] and build['cargo_artifact']['fresh'] is False
selected=gzip.decompress((p/'selected-legacy-bridge.rs.gz').read_bytes())
assert sha(selected)==build['source_sha256']['src/legacy_bridge.rs']
assert b'const WAIT: Duration = Duration::from_secs(10);' in selected
assert b'reason: "Channel Join Timeout".into()' in selected
before=(p/'before.log').read_text();assert 'ResetWithoutClosingHandshake' in before and '0 passed; 1 failed' in before
checks=(p/'tests.log').read_text()
assert 'test legacy_bridge::tests::frontend_join_timeout_closes_cleanly_and_recovers_the_only_slot ... ok' in checks
assert '137 passed; 0 failed' in checks
assert 'Finished' in (p/'clippy.log').read_text()
js=(p/'js.log').read_text();assert 'pass 65' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
print('PASS: pre-fix unclean EOF reproduced; selected timeout/recovery regression, 137 library and 65 JS checks, release source identity')
