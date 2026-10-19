"""Verify cached-slot scheduler screen and complete offered-load matrices."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
for line in (ROOT/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==expected,name
s=json.loads((ROOT/'summary.json').read_text());b=json.loads((ROOT/'builds.json').read_text())
assert s['selected'] is False
for row in s['small']:
    d=json.loads(gzip.decompress((ROOT/'raw'/row['file']).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code']==d['client_exit_code']==0
    assert d['server_sha256']==b['servers'][row['variant']]
    m=d['mixed'];n=m['healthy']['verified']+m['fanout']['verified']
    assert abs(row['server_cpu_seconds_per_million']-m['server_cpu_seconds']/n*1e6)<1e-12
for row in s['large']:
    stem=row['file'].removesuffix('.json.gz')
    d=json.loads(gzip.decompress((ROOT/'raw'/row['file']).read_bytes()))
    assert d['passed'] and not d['args']['profile'] and d['exit_codes']==[0,0,0]
    assert d['sha256']['server']==b['servers'][row['variant']]
    assert d['fanout_windows']=={'completed':96,'offered':96} and d['fanout_offered_load_met']
    assert row['server_cpu_seconds']==d['server_cpu_seconds']
    assert row['private']==d['latency'] and row['fanout']==d['fanout_exact_ms']
    for suffix,key,count,samples in [('.csv','csv_sha256',100000,24),('-fanout.csv','fanout_csv_sha256',8192,384)]:
        data=gzip.decompress((ROOT/'raw'/(stem+suffix+'.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==d[key]
        rows=list(csv.DictReader(io.StringIO(data.decode())))
        assert len(rows)==len({r['id']for r in rows})==count
        assert all(int(r['samples'])==samples for r in rows)
    data=gzip.decompress((ROOT/'raw'/(stem+'.windows.json.gz')).read_bytes())
    assert hashlib.sha256(data).hexdigest()==d['latency_windows_sha256']==d['latency_stages_windows_sha256']
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert d['final_metrics'][k]==0,k
assert '81 passed; 0 failed' in (ROOT/'raw/ready-slot-tests.log').read_text()
assert '2 passed; 0 failed' in (ROOT/'raw/ready-slot-arena-tests.log').read_text()
print('Checksums, binaries, tests, full offers, decoded user matrices and retirement verified.')
