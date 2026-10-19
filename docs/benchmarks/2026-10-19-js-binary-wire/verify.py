import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for name in ['original', 'selected']:
    assert hashlib.sha256((root / f'{name}-relay.mjs').read_bytes()).hexdigest() == meta[f'{name}_source_sha256']
original = (root / 'original-relay.mjs').read_text()
selected = (root / 'selected-relay.mjs').read_text()
helper = """function decodeMessage(data) {
  // A numeric string selects Uint8Array's length overload, not a byte view.
  if (typeof data === 'string') throw new TypeError('Relay requires binary CBOR messages');
  return decoder.decode(new Uint8Array(data));
}
"""
assert helper in selected and selected.count('decodeMessage(event.data)') == 2
assert selected.replace(helper, '', 1).replace('decodeMessage(event.data)', 'decoder.decode(new Uint8Array(event.data))') == original
before = (root / 'before.log').read_text()
assert 'fail 2' in before and '1048576' in before and 'typed-array length overload' in before
for file, count in [('js-final.log', 65), ('bundle-final.log', 32)]:
    text = (root / file).read_text()
    assert f'pass {count}' in text and 'fail 0' in text and 'skipped 0' in text
assert 'all six checked gauges are zero' in (root / 'js-final.log').read_text()
assets = json.loads((root / 'assets.json').read_text())
bundle = gzip.decompress((root / 'relay.js.gz').read_bytes())
assert hashlib.sha256(bundle).hexdigest() == assets['bundle_sha256'] == assets['served_sha256']
assert assets['served_status'] == 200 and assets['source_sha256'] == meta['selected_source_sha256']
assert b'Relay requires binary CBOR messages' in bundle
for file in ['bundle-connect.test.mjs', 'bundle-protocol-error.test.mjs']:
    assert 'globalThis.Buffer = undefined' in (root / file).read_text()
print('PASS: original allocation-overload failures; exact narrow source change; 65 integration and 32 built-bundle tests; served asset hash')
