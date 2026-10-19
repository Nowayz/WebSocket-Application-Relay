import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
summary=json.loads((p/'summary.json').read_text());assert not summary['selected'] and len(summary['cases'])==8
control=json.loads((p/'control-build.json').read_text());candidate=json.loads((p/'candidate-build.json').read_text())
for r in [control,candidate]:assert r['passed'] and not r['cargo_artifact']['fresh']
a,b=control['source_sha256'],candidate['source_sha256'];assert sorted(k for k in a.keys()|b.keys() if a.get(k)!=b.get(k))==['src/relay.rs','src/relay/listener_tests.rs']
for row in summary['cases']:
 r=json.loads(gzip.decompress((p/row['receipt']).read_bytes()));assert r['passed'] and r['correctness_passed'] and r['fanout_offered_load_met'] and r['exit_codes']==[0,0,0]
 assert row['private']==r['latency']['max_ms'] and row['fanout']==r['fanout_exact_ms']['max_ms'] and row['server_cpu_seconds']==r['server_cpu_seconds']
 assert row['churn_lifecycles']==r['sidecar_events'][-1]['details']['counts']['joins']-r['args']['readers']-2==256
 assert r['server_command'][r['server_command'].index('--workers')+1]=='8'
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:assert r['final_metrics'][k]==0
 if row['scale']=='100k':assert r['latency_sampling']['total_private_observations']==800000 and r['final_metrics']['war_native_connection_slots']==0
assert '111 passed; 0 failed' in (p/'tests.log').read_text()
assert 'observer output must release the membership registry' in (p/'counterfactual.log').read_text()
print('Verified eight mixed-load receipts, exact lifecycle counts, source isolation, tests and counterfactual.')
