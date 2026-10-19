"""Check saved lifecycle evidence without starting processes."""
import gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in json.loads((p/'sha256.json').read_text()).items():assert sha((p/name).read_bytes())==digest,name
build=json.loads((p/'build.json').read_text());assert build['passed'] and build['cargo_artifact']['fresh'] is False
selected=gzip.decompress((p/'selected-legacy-bridge.rs.gz').read_bytes())
assert sha(selected)==build['source_sha256']['src/legacy_bridge.rs']
assert selected.count(b'let mut pending = None;')==1
before=(p/'before.log').read_text();assert '0 passed; 2 failed' in before
for name in ['frontend_close_during_upstream_upgrade_releases_the_only_slot','frontend_eof_during_upstream_upgrade_releases_the_only_slot','frontend_ping_and_pipeline_during_upstream_upgrade_preserve_fifo']:
 assert f'test legacy_bridge::tests::{name} ... ok' in (p/'tests.log').read_text()
assert '136 passed; 0 failed' in (p/'tests.log').read_text()
assert 'Finished' in (p/'clippy.log').read_text()
js=(p/'js.log').read_text();assert 'pass 65' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
x=json.loads((p/'release.json').read_text());assert x['passed'] and x['exit_code']==0 and not x.get('forced_kill')
assert x['binary_sha256']==build['binary_sha256'] and x['baseline_fds']==x['final_fds']==10
assert [r['case'] for r in x['cases']]==['ping-then-close','eof']
assert all(r['upstream_eof'] and r['fds']==10 and r['elapsed_seconds']<2 for r in x['cases'])
print('PASS: reproduced Close/EOF failures, 136 library/65 JS checks, single pending slot, release cancellation and descriptor retirement')
