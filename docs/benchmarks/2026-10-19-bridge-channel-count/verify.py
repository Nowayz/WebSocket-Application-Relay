import gzip,hashlib,json,re,math
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
profiles={}
for kind in ['control','selected']:
    x=json.loads((r/(kind+'-heaps.json')).read_text());profiles[kind]=x
    b=json.loads((r/(kind+'-build.json')).read_text());assert b['passed'] and b['cargo_artifact']['fresh'] is False
    for row in x['runs']:
        assert row['binary_sha256']==b['binary_sha256']
        data=gzip.decompress((r/(kind+'-'+str(row['iterations'])+'.heap.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==row['heap_sha256']
        total=[0]*4
        for line in data.splitlines():
            a=re.match(rb'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\] @',line)
            if a:total=[v+int(w) for v,w in zip(total,a.groups())]
        assert total==row['counts']
        assert row['stdout']['entries']==30000 and row['stdout']['input_bytes']==7710007
    zero,work=x['runs'];assert zero['counts'][:2]==work['counts'][:2]
    assert (work['counts'][2]-zero['counts'][2])/64==x['allocated_objects_per_conversion']
    assert math.isclose((work['counts'][3]-zero['counts'][3])/64,x['allocated_bytes_per_conversion'])
assert profiles['control']['allocated_objects_per_conversion']==3
assert profiles['selected']['allocated_objects_per_conversion']==1
assert profiles['control']['allocated_bytes_per_conversion']>7800000
assert profiles['selected']['allocated_bytes_per_conversion']<73
source=gzip.decompress((r/'selected-bridge.rs.gz').read_bytes())
b=json.loads((r/'bridge-build.json').read_text());assert b['passed'] and b['cargo_artifact']['fresh'] is False
assert hashlib.sha256(source).hexdigest()==b['source_sha256']['src/legacy_bridge.rs']
assert b'(protocol::MAX_ITEMS - 3) / 3' in source
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
i=(r/'integration.log').read_text();assert 'ℹ pass 65' in i and 'all six checked gauges are zero' in i
wire=json.loads((r/'wire.json').read_text());assert wire['passed'] and wire['binary_sha256']==b['binary_sha256']
for case in wire['cases']:
    assert case['passed'] and case['exit_code']==0 and case['fd_baseline']==case['fd_final']==10
bad,good=wire['cases'];assert bad['frontend_body_hex']==(b'\x03\xebValue Requires CBOR Client').hex()
assert good['frontend_body_hex']=='00'*12
print('Channel-count guard verified: ~7.8 MB avoided per malformed conversion, valid item-budget boundary, 133 library/65 JS checks and unchanged release wire outcomes.')
