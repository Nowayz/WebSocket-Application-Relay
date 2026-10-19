"""Audit saved exact-ID regression and its isolated rounding mutant."""
import gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in json.loads((p/'sha256.json').read_text()).items():assert sha((p/name).read_bytes())==digest,name
base=gzip.decompress((p/'base-admin.rs.gz').read_bytes())
selected=gzip.decompress((p/'selected-admin.rs.gz').read_bytes())
mutant=gzip.decompress((p/'mutant-admin.rs.gz').read_bytes())
assert base.split(b'#[cfg(test)]',1)[0]==selected.split(b'#[cfg(test)]',1)[0]
assert selected.count(b'relay.disconnect(id)')==1
assert selected.replace(b'relay.disconnect(id)',b'relay.disconnect((id as f64) as u64)')==mutant
provenance=json.loads((p/'provenance.json').read_text())
assert sha(base)==provenance['base_admin_sha256']
assert sha(selected)==provenance['selected_admin_sha256']==provenance['source_sha256']['src/admin.rs']
assert sha(mutant)==provenance['mutant_admin_sha256']
assert '1 passed; 0 failed' in (p/'focused.log').read_text()
bad=(p/'mutant.log').read_text()
assert 'target 9007199254740993 did not receive administrative Close' in bad and '0 passed; 1 failed' in bad
suite=(p/'tests.log').read_text()
assert '138 passed; 0 failed' in suite
assert 'test admin::tests::disconnect_targets_exact_uint64_sessions_and_preserves_neighbours ... ok' in suite
assert 'Finished' in (p/'clippy.log').read_text() and not (p/'fmt.log').read_text()
print('PASS: test-only source change, three live ID boundary pairs, rounding mutant rejected, 138 library tests and strict checks')
