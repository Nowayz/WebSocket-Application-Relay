"""Verify real-context probe evidence at small and full population."""
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
    assert d['passed'] and not d['server_forced_kill']
    assert d['server_exit_code']==d['client_exit_code']==0
    assert d['client_sha256']==b['binaries'][row['variant']]
    m=d['mixed'];n=m['fanout']['verified']
    assert abs(row['client_cpu_seconds_per_million']-m['client_cpu_seconds']/n*1e6)<1e-12
for row in s['large']:
    name=row['file'].removesuffix('.json.gz')
    d=json.loads(gzip.decompress((ROOT/'raw'/row['file']).read_bytes()))
    assert d['passed'] and not d['args']['profile'] and d['exit_codes']==[0,0,0]
    assert d['fanout_windows']=={'completed':96,'offered':96} and d['fanout_offered_load_met']
    assert d['sha256']['sidecar']==b['binaries'][row['variant']]
    assert row['sidecar_cpu_seconds']==d['client_after']['sidecar']['cpu_seconds']-d['client_before']['sidecar']['cpu_seconds']
    assert row['private']==d['latency'] and row['fanout']==d['fanout_exact_ms']
    for suffix,key,count,samples in [('.csv','csv_sha256',100000,24),('-fanout.csv','fanout_csv_sha256',8192,384)]:
        data=gzip.decompress((ROOT/'raw'/(name+suffix+'.gz')).read_bytes())
        assert hashlib.sha256(data).hexdigest()==d[key]
        rows=list(csv.DictReader(io.StringIO(data.decode())))
        assert len(rows)==len({r['id']for r in rows})==count
        assert all(int(r['samples'])==samples for r in rows)
    data=gzip.decompress((ROOT/'raw'/(name+'.windows.json.gz')).read_bytes())
    assert hashlib.sha256(data).hexdigest()==d['latency_windows_sha256']==d['latency_stages_windows_sha256']
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert d['final_metrics'][k]==0,k
assert '10 passed; 0 failed' in (ROOT/'raw/context-read-tests.log').read_text()
for name,key in [('control-main.rs','control'),('context-main-built.rs','context_release')]:
    assert hashlib.sha256((ROOT/'raw'/name).read_bytes()).hexdigest()==b['main_sources'][key]
print('Checksums, frozen sources/binaries, small/full offers, decoded matrices and retirement verified.')
