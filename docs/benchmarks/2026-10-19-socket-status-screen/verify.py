import gzip, hashlib, json
from pathlib import Path
r=Path(__file__).resolve().parent
meta=json.loads((r/'provenance.json').read_text())
for name,digest in meta['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
old=gzip.decompress((r/'original-server.rs.gz').read_bytes()).decode()
expected=old.replace('let (fatal, requested) = {','let (fatal, requested, write_deadline) = {').replace('(state.fatal, state.requested)','(state.fatal, state.requested, state.deadline())').replace('if self.socket.deadline().is_some_and(|d| d <= now)','if write_deadline.is_some_and(|d| d <= now)')
assert expected==(r/'candidate-server.rs').read_text()
a=json.loads((r/'control-build.json').read_text());b=json.loads((r/'candidate-build.json').read_text())
assert a['passed'] and b['passed'] and a['cargo_artifact']['fresh'] is False and b['cargo_artifact']['fresh'] is False
assert {name for name in a['source_sha256'] if a['source_sha256'][name]!=b['source_sha256'][name]}=={'src/server.rs'}
assert a['binary_sha256']!=b['binary_sha256']
assert '126 passed; 0 failed' in (r/'tests.log').read_text()
comparison=json.loads((r/'comparison.json').read_text());assert comparison['passed']
for row in comparison['runs']:
    x=json.loads((r/(row['label']+'.json')).read_text())
    assert x['passed'] and x['correctness_passed'] and x['exit_codes']==[0,0,0]
    assert x['fanout_windows']=={'completed':32,'offered':32}
    assert x['server_cpu_seconds']==row['cpu']
    assert x['latency_sampling']['total_private_observations']==800000
    for name in ['war_connections','war_sessions','war_queued_bytes','war_native_connection_slots','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']:
        assert x['final_metrics'][name]==0,name
profile=json.loads((r/'profile.json').read_text());assert profile['passed'] and profile['profile']['data_bytes']>0
assert 'Total Lost Samples: 0' in (r/'profile-symbols.txt').read_text()
print('Unselected socket-status experiment verified: exact patch, fresh builds, 126 tests and four passing 100k screens.')
