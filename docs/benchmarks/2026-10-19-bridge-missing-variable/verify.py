import gzip
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
source = (root / 'legacy_bridge.rs').read_bytes()
assert hashlib.sha256(source).hexdigest() == meta['selected_source_sha256']
old = gzip.decompress((root / 'original-legacy_bridge.rs.gz').read_bytes()).decode()
assert 'd.null()?;\n                    &[]' not in old
assert 'd.null()?;\n                    &[]' in source.decode()
assert '1 failed' in (root / 'before.log').read_text()
assert '123 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
assert '65' in (root / 'js.log').read_text() and 'all six checked gauges are zero' in (root / 'js.log').read_text()
for name in ['control', 'selected']:
    build = json.loads((root / (name + '-build.json')).read_text())
    wire = json.loads((root / (name + '-wire.json')).read_text())
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    assert wire['passed'] and len(wire['cases']) == 2
    assert wire['binary_sha256'] == build['binary_sha256']
    if name == 'selected':
        assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
    for case in wire['cases']:
        assert case['passed'] and case['worker_finished'] and case['exit_code'] == 0
        assert not case.get('forced_kill')
        assert case['fd_baseline'] == case['fd_final'] == 10
        if name == 'selected' and not case['valid_missing_reply']:
            assert case['frontend_opcode'] == 8
            assert bytes.fromhex(case['frontend_body_hex']) == b'\x03\xebValue Requires CBOR Client'
            assert case['upstream_retirement'] == ['eof']
        else:
            assert case['frontend_opcode'] == 2
            assert case['frontend_body_hex'] == '0000000000000000c8'
            assert case['upstream_retirement'] == ['close-handshake']
print('Bridge missing-variable validation verified: failed regression, 123 library tests, 65 JS checks and four release-wire cases.')
