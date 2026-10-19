import gzip,hashlib,json,re
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
values={}
for label in ['control','selected']:
    x=json.loads((r/(label+'.json')).read_text());assert x['passed'] and x['exit_code']==0 and x['worker_finished'] and not x['upstream_errors']
    assert x['fd_baseline']==x['fd_final']==10
    values[label]={}
    for g in x['gates']:
        data=gzip.decompress((r/(label+'-'+g['phase']+'.heap.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==g['sha256']
        header=re.match(rb'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)',data)
        assert header
        total=[0]*4
        for line in data.splitlines():
            row=re.match(rb'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\] @',line)
            if row:
                total=[a+int(b) for a,b in zip(total,row.groups())]
        assert total==[int(v) for v in header.groups()]
        assert total[1]==g['live_bytes']
        values[label][g['phase']]=total[1]
old=values['control'];new=values['selected']
assert (old['closing_without_frontend_ack']-old['pending'])-(new['closing_without_frontend_ack']-new['pending'])==4*1024*1024
before=gzip.decompress((r/'original-bridge.rs.gz').read_bytes()).decode()
selected=gzip.decompress((r/'selected-bridge.rs.gz').read_bytes()).decode()
assert selected.count('drop(pending.take());')==8 and 'drop(pending.take());' not in before
assert before[before.index('#[cfg(test)]'):]==selected[selected.index('#[cfg(test)]'):]
b=json.loads((r/'build.json').read_text());assert b['passed'] and b['cargo_artifact']['fresh'] is False
assert hashlib.sha256(selected.encode()).hexdigest()==b['source_sha256']['src/legacy_bridge.rs']
assert b['binary_sha256']==json.loads((r/'selected.json').read_text())['binary_sha256']
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
integration=(r/'integration.log').read_text();assert 'ℹ pass 65' in integration and 'all six checked gauges are zero' in integration
print('Pending retention verified: 4 MiB requested-heap reduction at held Close, unchanged tests, 133 library/65 JS passes and retired descriptors.')
