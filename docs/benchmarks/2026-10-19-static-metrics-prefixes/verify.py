import gzip,hashlib,json,math,re,statistics
from pathlib import Path
r=Path(__file__).resolve().parent
meta=json.loads((r/'provenance.json').read_text())
for name,digest in meta['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
comparison=json.loads((r/'comparison.json').read_text());assert comparison['passed'] and len(comparison['runs'])==12
for row in comparison['runs']:
    data=(r/(row['case']+'.prom')).read_bytes();assert hashlib.sha256(data).hexdigest()==row['output_sha256']
    assert len(data)==row['length'] and statistics.median(row['ns_per_render'])==row['median_ns']
for case in ['zero','max','budget']:
    control=[x['median_ns'] for x in comparison['runs'] if x['case']==case and x['kind']=='control']
    candidate=[x['median_ns'] for x in comparison['runs'] if x['case']==case and x['kind']=='candidate']
    assert max(candidate)<min(control)
for prefix in ['helper','native']:
    builds={kind:json.loads((r/(prefix+'-'+kind+'-build.json')).read_text()) for kind in ['control','candidate']}
    for b in builds.values():assert b['passed'] and b['cargo_artifact']['fresh'] is False
    assert {name for name in builds['control']['source_sha256'] if builds['control']['source_sha256'][name]!=builds['candidate']['source_sha256'][name]}=={'src/metrics.rs'}
rows=json.loads((r/'allocations.json').read_text())
for row in rows:
    for g in row['captures']:
        data=gzip.decompress((r/(row['kind']+'-'+row['case']+'-'+str(g['iterations'])+'.heap.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==g['sha256']
        total=[0]*4
        for line in data.splitlines():
            match=re.match(rb'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\] @',line)
            if match:total=[a+int(b) for a,b in zip(total,match.groups())]
        assert total==g['counts']
    a,b=row['captures'];assert a['counts'][:2]==b['counts'][:2]
    assert (b['counts'][2]-a['counts'][2])/64==row['objects_per_render']
    assert math.isclose((b['counts'][3]-a['counts'][3])/64,row['bytes_per_render'])
    assert row['objects_per_render']==(2 if row['case']=='max' else 1)
for case in ['zero','max','budget']:
    a,b=[x for x in rows if x['case']==case];assert a['objects_per_render']==b['objects_per_render'] and a['bytes_per_render']==b['bytes_per_render']
http=json.loads((r/'http.json').read_text());assert http['passed']
for row in http['runs']:
    assert row['passed'] and row['exit_code']==0
    for state in row['states']:
        assert hashlib.sha256((r/('http-'+state['state']+'.prom')).read_bytes()).hexdigest()==state['sha256']
        assert len(state['values'])==43 and state['values']['war_draining']==int(state['state']=='draining')
old=(r/'original-metrics.rs').read_text();new=(r/'selected-metrics.rs').read_text()
pattern=r'        for \(([^)]+)\) in \[(.*?)\n        \] \{(.*?)\n        \}'
a=[];b=[]
for i,m in enumerate(re.finditer(pattern,old,re.S)):
    entries=re.findall(r'\(\s*"([a-z_]+)",\s*(?:"(gauge|counter)",\s*)?(.*?),\s*"([^"\n]*)",\s*\)',m[2],re.S)
    a.append([(name,kind or ('counter' if i==1 else 'gauge'),re.sub(r'\s+','',value),helptext) for name,kind,value,helptext in entries])
for m in re.finditer(pattern,new,re.S):
    entries=re.findall(r'sample!\(\s*"([a-z_]+)",\s*"(gauge|counter)",\s*(.*?),\s*"([^"\n]*)"\s*,?\s*\)',m[2],re.S)
    b.append([(name,kind,re.sub(r'\s+','',value),helptext) for name,kind,value,helptext in entries])
assert a==b and list(map(len,a))==[4,30,9]
assert '133 passed; 0 failed' in (r/'tests.log').read_text()
assert 'Finished `dev` profile' in (r/'clippy.log').read_text()
i=(r/'integration.log').read_text();assert 'ℹ pass 65' in i and 'all six checked gauges are zero' in i
print('Static metrics prefixes verified: 12 exact render outputs, unchanged definitions/load groups and allocation counts, native HTTP states, 133 library/65 JS checks.')
