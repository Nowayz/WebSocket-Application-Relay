import gzip, hashlib, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()
builds={k:json.loads(read(k+'-build.json')) for k in ['control','candidate','profile']}
assert all(b['passed'] and not b['cargo_artifact']['fresh'] for b in builds.values())
a,b=[builds[k]['source_sha256'] for k in ['control','candidate']]
assert [k for k in a if a[k]!=b[k]]==['src/server/socket.rs']
rows=json.loads((root/'summary.json').read_text());assert len(rows)==8
clients=set()
for kind in ['churn-mixed','private']:
    assert [r['variant'] for r in rows if r['workload']==kind]==['control','candidate','candidate','control']
for row in rows:
    x=json.loads(read(row['receipt']));clients.add(x['client_sha256'])
    assert x['passed'] and not x['profile_diagnostic']
    assert x['server_exit_code']==x['client_exit_code']==0 and not x['server_forced_kill']
    assert x['server_sha256']==builds[row['variant']]['binary_sha256']
    assert x['args']['seconds']==2 and x['args']['pairs']==(1 if row['workload']=='churn-mixed' else 64)
    assert x['args']['churn_workers']==(8 if row['workload']=='churn-mixed' else 0)
    m=x['mixed'];n=m['healthy']['verified']+m['churn']['verified']
    assert row['cpu_per_million_operations']==m['server_cpu_seconds']/n*1e6
    assert row['private_max_ms']==m['healthy']['max_ms'] and row['churn_max_ms']==m['churn']['max_ms']
    done=next(e for e in x['events'] if e['event']=='done');assert done['details']['passed']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert done['metrics'][key]==0,key
assert len(clients)==1
p=json.loads(read('byte-validation-profile.json'));assert p['passed'] and p['profile_diagnostic']
assert p['server_sha256']==builds['profile']['binary_sha256']
assert 'Total Lost Samples: 0' in read('byte-validation-profile-detail.txt')
assert 'protocol::validate_value' in read('byte-validation-profile-detail.txt') and 'memset' in read('byte-validation-entry.asm')
failed=json.loads(read('single-slice-churn-0-control.json'));assert not failed['passed'] and 'invalid workload bounds' in failed['client_log']
assert '92 passed; 0 failed' in read('single-slice-tests.log')
assert 'Finished' in read('single-slice-clippy.log') and 'error:' not in read('single-slice-clippy.log')
print('Eight short ordinary trials, source/binary identities, counters/retirement, 92 library tests/Clippy, failed fixture and separate parser profile verified.')
