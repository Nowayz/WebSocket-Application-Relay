import gzip,hashlib,json,math
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
control=gzip.decompress((r/'control-inspection.rs.gz').read_bytes())
candidate=gzip.decompress((r/'candidate-inspection.rs.gz').read_bytes())
assert control.replace(b'read_buffer_size(4096)',b'read_buffer_size(8192)')==candidate
builds=[json.loads((r/(kind+'-build.json')).read_text()) for kind in ['control','candidate']]
for b in builds:
    assert b['passed'] and b['cargo_artifact']['fresh'] is False
assert {key for key in builds[0]['source_sha256'] if builds[0]['source_sha256'][key]!=builds[1]['source_sha256'][key]}=={'examples/inspection.rs'}
comparison=json.loads((r/'comparison.json').read_text());assert comparison['passed'] and len(comparison['runs'])==4
server_hashes=set()
for row in comparison['runs']:
    x=json.loads((r/(row['label']+'.json')).read_text());assert x['passed'] and x['client_exit_code']==x['server_exit_code']==0 and not x['server_forced_kill']
    server_hashes.add(x['server_sha256'])
    mixed=x['mixed'];n=mixed['fanout']['verified'];assert n==row['deliveries']
    assert math.isclose(mixed['client_cpu_seconds']*1e6/n,row['client_cpu_per_million'])
    assert math.isclose(mixed['server_cpu_seconds']*1e6/n,row['server_cpu_per_million'])
    metrics=x['events'][-1]['metrics']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_native_connection_slots','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert metrics[key]==0,key
assert len(server_hashes)==1
controls=[row for row in comparison['runs'] if row['kind']=='control'];candidates=[row for row in comparison['runs'] if row['kind']=='candidate']
assert min(row['client_cpu_per_million'] for row in candidates)>max(row['client_cpu_per_million'] for row in controls)
assert min(row['fanout']['max_ms'] for row in candidates)>max(row['fanout']['max_ms'] for row in controls)
assert max(row['fanout']['per_second'] for row in candidates)<min(row['fanout']['per_second'] for row in controls)
assert '9 passed; 0 failed' in (r/'tests.log').read_text()
print('8 KiB client buffer rejected: exact one-setting source boundary, fresh builds, nine tests, four passing screens and worse ordinary CPU/latency/throughput.')
