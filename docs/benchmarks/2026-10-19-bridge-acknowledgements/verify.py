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
old = gzip.decompress((root / 'original-legacy_bridge.rs.gz').read_bytes())
assert b'0 | 1 | 3 | 4 => None' in old and b'0 | 1 | 3 | 4 => None' not in source
for guard in [b'd.u8()? <= 7', b'id != 0 && id != BROADCAST', b'ensure!(d.bool()?']:
    assert guard in source
assert '1 passed; 1 failed' in (root / 'before.log').read_text()
assert '125 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
assert '65' in (root / 'js.log').read_text() and 'all six checked gauges are zero' in (root / 'js.log').read_text()
for label in ['control', 'selected']:
    build = json.loads((root / (label + '-build.json')).read_text())
    wire = json.loads((root / (label + '-wire.json')).read_text())
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    assert wire['passed'] and wire['binary_sha256'] == build['binary_sha256']
    if label == 'selected':
        assert build['source_sha256']['src/legacy_bridge.rs'] == meta['selected_source_sha256']
    assert len(wire['cases']) == 8
    assert {(r['opcode'], r['valid_acknowledgement']) for r in wire['cases']} == {(op, valid) for op in [0, 1, 3, 4] for valid in [False, True]}
    for case in wire['cases']:
        assert case['passed'] and case['worker_finished'] and case['exit_code'] == 0
        assert not case.get('forced_kill') and case['fd_baseline'] == case['fd_final'] == 10
        if label == 'selected' and not case['valid_acknowledgement']:
            assert case['frontend_opcode'] == 8
            assert bytes.fromhex(case['frontend_body_hex']) == b'\x03\xebValue Requires CBOR Client'
            assert case['upstream_retirement'] in [['eof'], ['reset']]
        else:
            assert case['frontend_opcode'] == 2 and case['frontend_body_hex'] == '0000000000000000c8'
            assert case['upstream_retirement'] == ['close-handshake']
print('Suppressed acknowledgement validation verified: 125 library tests, 65 JS checks, 16 frozen release-wire cases.')
