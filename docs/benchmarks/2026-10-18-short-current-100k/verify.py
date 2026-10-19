import csv, gzip, hashlib, io, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()
summary=json.loads((root/'summary.json').read_text())
assert [r['variant'] for r in summary]==['baseline','current','current','baseline']
server_hashes={}; clients=set()
for row in summary:
    x=json.loads(read(row['receipt']));stem=row['receipt'].removesuffix('.json')
    assert x['passed'] and x['exit_codes']==[0,0,0]
    assert x['args']['users']==100000 and x['args']['seconds']==2 and x['args']['readers']==8192
    assert x['args']['inspectors']==x['args']['pairs']==x['args']['churn_cycles_per_second']==0
    assert row['overall_workload_passed']==(x['passed'] and x['fanout_offered_load_met'])
    assert x['fanout_windows']==row['windows'] and row['cpu_seconds']==x['server_cpu_seconds']
    server_hashes.setdefault(row['variant'],x['sha256']['server']);assert server_hashes[row['variant']]==x['sha256']['server']
    clients.add((x['sha256']['capacity_client'],x['sha256']['sidecar']))
    assert hashlib.sha256(read('original-runner.py').encode()).hexdigest()==x['sha256']['runner']
    for suffix,key in [('.csv','csv_sha256'),('-fanout.csv','fanout_csv_sha256')]:
        text=read(stem+suffix);assert hashlib.sha256(text.encode()).hexdigest()==x[key]
        users=list(csv.DictReader(io.StringIO(text)))
        expected_count=100000 if suffix=='.csv' else 8192
        expected_samples=8 if suffix=='.csv' else row['windows']['completed']*4
        assert len(users)==len({u['id'] for u in users})==expected_count
        assert all(int(u['samples'])==expected_samples for u in users)
        for column in (['max_ms','schedule_max_ms'] if suffix=='.csv' else ['max_ns','schedule_max_ns']):
            values=sorted(float(u[column])/(1e6 if suffix!='.csv' else 1) for u in users)
            metric=column.replace('_ns','_ms')
            observed=x['latency'][metric] if suffix=='.csv' else x['fanout_exact_ms'][metric]
            for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:
                assert values[(len(values)-1)*q//100]==observed[label]
    assert not x['generator_lag']['exceeded_one_period']
    final=x['final_metrics']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']:
        assert final[key]==0,key
    assert final['war_channels']==1
    left=x['events'][-1]['counts'];right=x['sidecar_events'][-1]['details']['counts']
    for metric,field in [('war_accepted_total','joins'),('war_joined_total','joins'),('war_disconnected_total','normal_closes'),('war_incoming_messages_total','sent_messages'),('war_incoming_bytes_total','sent_bytes'),('war_deliveries_total','deliveries'),('war_delivered_bytes_total','delivered_bytes'),('war_routing_reused_bytes_total','value_bytes')]:
        assert final[metric]==left[field]+right[field],metric
    for key,value in final.items():
        if any(word in key for word in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','aborted_writes_total','close_failures_total','transport_disconnects_total','global_pressure_total']):
            assert value==0,key
    assert x['latency_sampling']['total_private_observations']==800000
assert len(clients)==1 and [r['overall_workload_passed'] for r in summary]==[True,False,True,True]
assert json.loads(read('current-build.json'))['binary_sha256']==server_hashes['current']
log=read('mixed-load-gate-tests.log')
assert '"rate": 1000000, "exit_code": 1, "passed": false, "correctness_passed": true' in log
assert '"rate": 1, "exit_code": 0, "passed": true' in log
assert '"rate": 0, "exit_code": 0, "passed": true' in log
assert 'OK' in log and 'OK' in read('mixed-load-gate-output-tests.log')
print('Four complete-population receipts, 3.2 million private observations, exact fanout samples, missed-load status, plots and real runner-gate regressions verified.')
