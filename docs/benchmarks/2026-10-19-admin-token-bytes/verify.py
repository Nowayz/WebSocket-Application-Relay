import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
build = json.loads((root / 'build.json').read_text())
old = gzip.decompress((root / 'original-admin.rs.gz').read_bytes())
new = (root / 'admin.rs').read_bytes()
assert hashlib.sha256(old).hexdigest() == meta['original_source_sha256']
assert hashlib.sha256(new).hexdigest() == meta['selected_source_sha256'] == build['source_sha256']['src/admin.rs']
assert build['passed'] and build['cargo_artifact']['fresh'] is False
old_chain = """        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|s| {
            protocol::secret_matches(relay.config.admin_token.as_deref(), s.as_bytes())
        })"""
new_chain = """        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .is_some_and(|s| protocol::secret_matches(relay.config.admin_token.as_deref(), s))"""
assert new.decode().split('#[cfg(test)]', 1)[0].replace(new_chain, old_chain, 1) == old.decode().split('#[cfg(test)]', 1)[0]
before = (root / 'before-all.log').read_text()
assert '2 failed' in before and 'authorization_compares_exact_configured_token_bytes ... FAILED' in before
assert 'unicode_admin_token_authenticates_real_http_without_text_coercion ... FAILED' in before
text = (root / 'tests.log').read_text()
assert '116 passed; 0 failed' in text and '2 passed; 0 failed' in text
assert 'Finished `dev` profile' in (root / 'clippy.log').read_text()
js = (root / 'js.log').read_text()
assert 'pass 65' in js and 'fail 0' in js and 'skipped 0' in js
assert 'all six checked gauges are zero' in js
p = json.loads((root / 'release.json').read_text())
assert p['passed'] and len(p['cases']) == 4
for case in p['cases']:
    assert case['passed'] and case['exit_code'] == 0
    if case['kind'] == 'selected':
        assert case['binary_sha256'] == build['binary_sha256']
    allowed = case['kind'] == 'selected' or case['token_kind'] == 'ascii'
    expected = [401, 401, 401, 401, 401, 200 if allowed else 401, 200 if allowed else 401, 204 if allowed else 401, 503 if allowed else 200, 204 if allowed else 401, 200]
    assert [x['status'] for x in case['statuses']] == expected
print('PASS: exact narrow authorization change; both original regressions; 116 library/2 startup/65 JS tests; 44 release HTTP checks')
