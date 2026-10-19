import gzip
import hashlib
import json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
source = gzip.decompress((root / 'legacy_bridge.rs.gz').read_bytes())
assert hashlib.sha256(source).hexdigest() == meta['selected_source_sha256']
assert b'invalid upstream departed ID' in source
assert b'invalid upstream departed ID' not in gzip.decompress((root / 'original-legacy_bridge.rs.gz').read_bytes())
assert '1 failed' in (root / 'before.log').read_text()
assert '126 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
assert '65' in (root / 'js.log').read_text() and 'all six checked gauges are zero' in (root / 'js.log').read_text()
for label in ['control', 'selected']:
    build = json.loads((root / (label + '-build.json')).read_text())
    wire = json.loads((root / (label + '-wire.json')).read_text())
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    assert wire['passed'] and wire['binary_sha256'] == build['binary_sha256']
    assert len(wire['cases']) == 4
    assert {r['departed_id'] for r in wire['cases']} == {0, 2**64-1, 1, 2**64-2}
    if label == 'selected': assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
    for case in wire['cases']:
        assert case['passed'] and case['worker_finished'] and case['exit_code'] == 0
        assert not case.get('forced_kill') and case['fd_baseline'] == case['fd_final'] == 10
        if label == 'selected' and case['departed_id'] in [0, 2**64-1]:
            assert case['frontend_opcode'] == 8
            assert bytes.fromhex(case['frontend_body_hex']) == b'\x03\xebValue Requires CBOR Client'
            assert case['upstream_retirement'] in [['eof'], ['reset']]
        else:
            assert case['frontend_opcode'] == 2
            assert bytes.fromhex(case['frontend_body_hex']) == b'\xff'*8 + case['departed_id'].to_bytes(8,'little')
            assert case['upstream_retirement'] == ['close-handshake']
print('Departure validation verified: 126 library tests, 65 JS checks and eight frozen release-wire cases.')
