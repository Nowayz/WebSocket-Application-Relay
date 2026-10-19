import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
summary=json.loads((p/'summary.json').read_text());build=json.loads((p/'build.json').read_text());assert build['passed'] and not build['cargo_artifact']['fresh']
for i,n in enumerate([8,4,4,8]):
 r=json.loads(gzip.decompress((p/f'{i}-{n}w.json.gz').read_bytes()));s=summary[i]
 assert r['correctness_passed'] and r['exit_codes']==[0,0,0]
 assert r['args']['workers']==n and r['server_command'][r['server_command'].index('--workers')+1]==str(n)
 assert len(r['affinity'])==n and all(len(t['cpus'])==1 for t in r['affinity']) and len({t['cpus'][0] for t in r['affinity']})==n
 assert r['sha256']['server']==build['binary_sha256']
 assert r['passed']==r['fanout_offered_load_met']==(n==8) and r['fanout_windows']['offered']==32
 assert r['fanout_windows']['completed']==[32,19,20,32][i]
 assert s['private']==r['latency']['max_ms'] and s['fanout']==r['fanout_exact_ms']['max_ms'] and s['cpu_seconds']==r['server_cpu_seconds']
 assert r['latency_sampling']['total_private_observations']==800000
 assert r['sidecar_events'][-1]['details']['counts']['joins']-r['args']['readers']-2==s['churn_completed']==[256,166,174,256][i]
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:assert r['final_metrics'][k]==0
 for k,v in r['final_metrics'].items():
  if any(x in k for x in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','aborted_writes_total','close_failures_total','transport_disconnects_total','global_pressure_total']):assert v==0
assert 'Ran 2 tests' in (p/'cli-tests.log').read_text() and 'OK' in (p/'cli-tests.log').read_text()
print('Verified same-binary worker comparison, distinct affinities, retained overload failures, exact lifecycle counts and cleanup.')
