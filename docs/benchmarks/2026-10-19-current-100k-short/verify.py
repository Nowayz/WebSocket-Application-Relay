import csv,gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
root=p.parents[2]
summary=json.loads((p/'summary.json').read_text())
names=['control','candidate','repeat-candidate','repeat-control']
checked=0
for name,row in zip(names,summary):
 r=json.loads(gzip.decompress((p/(name+'.json.gz')).read_bytes()))
 assert r['passed'] and r['correctness_passed'] and r['fanout_offered_load_met'] and r['exit_codes']==[0,0,0]
 assert r['args']['users']==100000 and r['args']['readers']==8192 and r['args']['seconds']==2 and not r['args']['profile']
 assert r['server_command'][r['server_command'].index('--workers')+1]=='8'
 assert r['ready_metrics']['war_connections']==108194
 assert row['private']==r['latency']['max_ms'] and row['fanout']==r['fanout_exact_ms']['max_ms'] and row['server_cpu_seconds']==r['server_cpu_seconds']
 assert r['latency_sampling']['total_private_observations']==800000
 final=r['final_metrics']
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:assert final[k]==0
 for k,v in final.items():
  if any(s in k for s in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','aborted_writes_total','close_failures_total','transport_disconnects_total','global_pressure_total']):assert v==0
 for kind,meta in row['raw'].items():
  assert meta['sha256']==r['csv_sha256' if kind=='private' else 'fanout_csv_sha256']
  source=root/meta['path']
  if not source.exists():continue
  data=source.read_bytes();assert hashlib.sha256(data).hexdigest()==meta['sha256'];rows=list(csv.DictReader(data.decode().splitlines()))
  assert len(rows)==meta['users'] and len({x['id'] for x in rows})==meta['users']
  assert all(int(x['samples'])==meta['observations_per_user'] for x in rows)
  values=sorted(float(x['max_ms']) if kind=='private' else int(x['max_ns'])/1e6 for x in rows)
  for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:assert abs(values[(len(values)-1)*q//100]-row[kind][label])<1e-9
  for t,v in meta['under_ms_percent'].items():assert sum(x<=float(t) for x in values)*100/len(values)==v
  checked+=1
print(f'Verified four receipts, counters, summaries and {checked}/8 local full-population CSVs.')
