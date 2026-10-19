import gzip, hashlib, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()
builds={k:json.loads(read(k+'-build.json')) for k in ['control','candidate']}
for kind,b in builds.items():
    assert b['passed'] and not b['cargo_artifact']['fresh']
    assert hashlib.sha256(read(kind+'-fanout.rs').encode()).hexdigest()==b['source_sha256']['examples/inspection/fanout.rs']
a,b=[builds[k]['source_sha256'] for k in ['control','candidate']]
assert [k for k in a if a[k]!=b[k]]==['examples/inspection/fanout.rs']
rows=json.loads((root/'summary.json').read_text())
assert [r['variant'] for r in rows]==['control','candidate','candidate','control']
server_hashes=set()
for row in rows:
    x=json.loads(read(row['receipt']));server_hashes.add(x['server_sha256'])
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code']==x['server_exit_code']==0 and not x['server_forced_kill']
    assert x['client_sha256']==builds[row['variant']]['binary_sha256']
    assert x['args']['seconds']==2 and x['args']['pairs']==64 and x['args']['fanout_readers']==2048
    m=x['mixed'];n=m['healthy']['verified']+m['fanout']['verified']
    assert row['cpu_per_million']==m['server_cpu_seconds']/n*1e6
    assert row['fanout_max_ms']==m['fanout']['max_ms'] and row['private_max_ms']==m['healthy']['max_ms']
    assert row['fanout_per_second']==m['fanout']['per_second']
    done=next(e for e in x['events'] if e['event']=='done');assert done['details']['passed']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert done['metrics'][key]==0,key
assert len(server_hashes)==1
for kind in ['private','fanout']:
    old=json.loads(read('previous-'+kind+'.json'))
    new=json.loads((root.parent/'2026-10-18-short-current-100k'/f'{kind}.json').read_text())
    assert old['cases']==new['cases'] and old['statistic']==new['statistic']
    assert 'measurement_start' in new
assert '8 passed; 0 failed' in read('fanout-send-first-tests.log')
assert 'Ran 9 tests' in read('fanout-send-first-plot-tests.log') and 'OK' in read('fanout-send-first-plot-tests.log')
print('Four short client-order trials, source/binary identities, counter retirement, eight example/nine plot tests and unchanged 100k figure statistics verified.')
