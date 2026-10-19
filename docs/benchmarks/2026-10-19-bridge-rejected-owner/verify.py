import gzip,hashlib,json,re
from pathlib import Path
r=Path(__file__).resolve().parent
meta=json.loads((r/'provenance.json').read_text())
for name,digest in meta['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
builds={k:json.loads((r/(k+'-build.json')).read_text()) for k in ['control','selected']}
for b in builds.values():assert b['passed'] and b['cargo_artifact']['fresh'] is False
assert {key for key in builds['control']['source_sha256'] if key.startswith('src/') and builds['control']['source_sha256'][key]!=builds['selected']['source_sha256'][key]}=={'src/legacy_bridge.rs'}
for kind in ['binary','text']:
    for mode in ['fragmented','single']:
        states=[]
        for variant in ['control','selected']:
            name=f'{variant}-{kind}-{mode}'
            x=json.loads((r/(name+'.json')).read_text())
            assert x['passed'] and x['exit_code']==0 and x['worker_finished'] and not x['upstream_errors']
            assert x['fd_baseline']==x['fd_final']==10
            assert x['binary_sha256']==builds[variant]['binary_sha256']
            expected=b'\x03\xebValue Requires CBOR Client' if kind=='binary' else b'\x03\xeaInvalid Upstream Message'
            assert x['frontend_close_body_hex']==expected.hex()
            assert x['input_message_bytes']==4*1024*1024+(10 if kind=='binary' else 0)
            state={}
            for g in x['gates']:
                data=gzip.decompress((r/(name+'-'+g['phase']+'.heap.gz')).read_bytes())
                assert hashlib.sha256(data).hexdigest()==g['sha256']
                header=re.match(rb'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)',data);assert header
                total=[0]*4
                for line in data.splitlines():
                    row=re.match(rb'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\] @',line)
                    if row:total=[a+int(b) for a,b in zip(total,row.groups())]
                assert total==list(map(int,header.groups())) and total[1]==g['live_bytes']
                state[g['phase']]=g['live_bytes']
            states.append(state)
        old,new=states
        delta=(old['closing_without_frontend_ack']-old['retired'])-(new['closing_without_frontend_ack']-new['retired'])
        assert delta==(4*1024*1024+(10 if kind=='binary' else 0) if mode=='fragmented' else 0)
old=gzip.decompress((r/'original-bridge.rs.gz').read_bytes()).decode()
new=gzip.decompress((r/'selected-bridge.rs.gz').read_bytes()).decode()
expected=old.replace('Err(_) => { drop(pending.take()); transport::initiate_close','Err(_) => { drop(bytes); drop(pending.take()); transport::initiate_close').replace('Some(Ok(Message::Text(_))) => {\n                    drop(pending.take());','Some(Ok(Message::Text(text))) => {\n                    drop(text);\n                    drop(pending.take());')
assert expected==new
assert hashlib.sha256(old.encode()).hexdigest()==builds['control']['source_sha256']['src/legacy_bridge.rs']
assert hashlib.sha256(new.encode()).hexdigest()==builds['selected']['source_sha256']['src/legacy_bridge.rs']
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
i=(r/'integration.log').read_text();assert 'ℹ pass 65' in i and 'all six checked gauges are zero' in i
print('Rejected-owner release verified: exact fragmented-owner heap reductions, zero single-frame reduction, eight clean release cases and 133 library/65 JS checks.')
