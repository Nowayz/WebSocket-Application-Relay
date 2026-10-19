import gzip,hashlib,json,math,re,statistics
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
component=json.loads((r/'component.json').read_text());http=json.loads((r/'http.json').read_text())
assert component['passed'] and http['passed'] and len(component['runs'])==len(http['runs'])==4
for row in component['runs']:
    for case in row['cases']:assert statistics.median(case['ns_per_operation'])==row['median_ns'][case['case']]
    assert '1 passed; 0 failed' in (r/Path(row['log']).name).read_text()
for prefix in ['component','native']:
    builds={k:json.loads((r/(prefix+'-'+k+'-build.json')).read_text()) for k in ['control','candidate']}
    for b in builds.values():
        assert b['passed']
        a=b.get('artifact',b.get('cargo_artifact'));assert a['fresh'] is False
    assert {k for k in builds['control']['source_sha256'] if builds['control']['source_sha256'][k]!=builds['candidate']['source_sha256'][k]}=={'src/relay/listing.rs'}
for row in http['runs']:
    x=json.loads((r/(row['label']+'.json')).read_text());assert x['passed'] and x['client_exit_code']==x['server_exit_code']==0 and not x['server_forced_kill']
    assert x['mixed']['inspection']['verified']==400
    assert x['mixed']['healthy']==row['healthy'] and x['mixed']['inspection']==row['inspection']
    for name in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_native_connection_slots','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert x['events'][-1]['metrics'][name]==0,name
heaps=json.loads((r/'heaps.json').read_text());assert len(heaps)==2
for row in heaps:
    for index,g in enumerate(row['captures']):
        data=gzip.decompress((r/(row['kind']+'-'+str(index)+'.heap.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==g['sha256']
        total=[0]*4
        for line in data.splitlines():
            a=re.match(rb'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\] @',line)
            if a:total=[v+int(w) for v,w in zip(total,a.groups())]
        assert total==g['counts']
    a,b=row['captures'];assert [y-x for x,y in zip(a['counts'],b['counts'])]==[0,0,1831,262080681]
source=(r/'selected-listing.rs').read_bytes()
assert b'.encode_slice(name, &mut key)' in source and b'name: SnapshotName::new(name)' in source
assert source.index(b'.encode_slice(name')<source.index(b'if after.is_some_and')<source.index(b'name: SnapshotName::new(name)')
assert b'ceiling' not in source and b'cutoff' not in source
b=json.loads((r/'native-candidate-build.json').read_text());assert hashlib.sha256(source).hexdigest()==b['source_sha256']['src/relay/listing.rs']
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
i=(r/'integration.log').read_text();assert 'ℹ pass 65' in i and 'all six checked gauges are zero' in i
print('Borrowed page keys verified: component medians, unchanged allocation traffic, four concurrent acceptance runs and 133 library/65 JS checks.')
