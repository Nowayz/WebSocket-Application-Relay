"""Verify equal-size typed workload acceptance and diagnostic evidence offline."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
for line in (ROOT/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==expected,name
s=json.loads((ROOT/'summary.json').read_text());b=json.loads((ROOT/'raw/build.json').read_text())
assert s['selected']
for row in s['rows']:
    d=json.loads(gzip.decompress((ROOT/'raw'/row['file']).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code']==d['client_exit_code']==0
    assert d['client_sha256']==b['binary_sha256']
    assert d['events'][0]['details']['value_kind']==d['mixed']['value_kind']==row['kind']
    m=d['mixed'];n=m['healthy']['verified']+m['fanout']['verified']
    assert abs(row['server_cpu_seconds_per_million_deliveries']-m['server_cpu_seconds']/n*1e6)<1e-12
    assert ('--value-kind' in d['command'])==(row['kind']!='bytes')
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert d['events'][-1]['metrics'][k]==0,k
for name in ['value-kind-old-client-check','value-kind-default-check','value-kind-utf8-profile']:
    d=json.loads(gzip.decompress((ROOT/'raw'/(name+'.json.gz')).read_bytes()))
    assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0 and not d['server_forced_kill']
    if name=='value-kind-old-client-check':
        assert '--value-kind' not in d['command']
        assert d['client_sha256']==b['previous_default_sha256']
    else:
        assert d['client_sha256']==b['binary_sha256']
x=json.loads(gzip.decompress((ROOT/'raw/value-kind-mixed-smoke.json.gz').read_bytes()))
assert x['passed'] and x['exit_codes']==[0,0,0]
assert x['fanout_windows']=={'completed':32,'offered':32} and x['fanout_offered_load_met']
assert x['sha256']['sidecar']==b['binary_sha256']
assert all(e['details']['value_kind']=='utf8' for e in x['sidecar_events'] if e['event']in ['ready','measured'])
for suffix,key in [('.csv','csv_sha256'),('-fanout.csv','fanout_csv_sha256'),('.windows.json','latency_windows_sha256')]:
    data=gzip.decompress((ROOT/'raw'/('value-kind-mixed-smoke'+suffix+'.gz')).read_bytes())
    assert hashlib.sha256(data).hexdigest()==x[key]
assert '8 passed; 0 failed' in (ROOT/'raw/value-kind-tests.log').read_text()
report=(ROOT/'raw/utf8-profile.report.txt').read_text()
assert '42.68%' in report and 'core::str::converts::from_utf8' in report and 'Total Lost Samples: 0' in report
bad=json.loads((ROOT/'raw/raw-wire-rejection.json').read_text())
assert bad['exit_code']!=0 and 'text value kinds require CBOR wire' in bad['stderr']
print('Checksums, active value kinds, default compatibility, decoded smoke matrices and UTF-8 profile verified.')
