"""Offline verification of short fairness receipts and decoded exact samples."""
import csv,gzip,hashlib,io,json
from pathlib import Path
root=Path(__file__).resolve().parent
for name,h in json.loads((root/'manifest.json').read_text()).items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
pairs=[('war_accepted_total','joins'),('war_joined_total','joins'),('war_disconnected_total','normal_closes'),('war_incoming_messages_total','sent_messages'),('war_incoming_bytes_total','sent_bytes'),('war_deliveries_total','deliveries'),('war_delivered_bytes_total','delivered_bytes'),('war_routing_reused_bytes_total','value_bytes')]
for prefix in ['short-fairness-refresh-01']+[f'short-fairness-quantum-{i}-{q}' for i,q in enumerate([1024,512,512,1024])]+['short-fairness-sampling-check']:
 d=json.loads((root/'raw'/(prefix+'.json')).read_text());assert d['passed'] and d['exit_codes']==[0,0,0]
 raw=gzip.decompress((root/'raw'/(prefix+'.csv.gz')).read_bytes());assert hashlib.sha256(raw).hexdigest()==d['csv_sha256']
 rows=list(csv.DictReader(io.StringIO(raw.decode())));assert len(rows)==len({r['id'] for r in rows})==d['args']['users'];assert all(int(r['samples'])==d['args']['seconds']*4 for r in rows)
 assert all(float(r['p99_ms'])==float(r['max_ms']) for r in rows)
 for col,stats in d['latency'].items():
  vals=sorted(float(r[col]) for r in rows)
  for name,q in [('median',50),('p95',95),('p99',99),('max',100)]:assert vals[(len(vals)-1)*q//100]==stats[name]
 if 'fanout_csv_sha256' in d:
  raw=gzip.decompress((root/'raw'/(prefix+'-fanout.csv.gz')).read_bytes());assert hashlib.sha256(raw).hexdigest()==d['fanout_csv_sha256']
  audience=list(csv.DictReader(io.StringIO(raw.decode())));assert len(audience)==len({r['id'] for r in audience})==d['args']['readers']
  measured=next(e for e in d['sidecar_events'] if e['event']=='measured')
  assert sum(int(r['samples']) for r in audience)==measured['details']['fanout']['verified']
 for suffix,key in [('.windows.json','latency_windows_sha256')]:
  if key in d:assert hashlib.sha256((root/'raw'/(prefix+suffix)).read_bytes()).hexdigest()==d[key]
 m=d['final_metrics'];assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']);assert m['war_channels']==1
 a=d['events'][-1]['counts'];b=d['sidecar_events'][-1]['details']['counts']
 for metric,field in pairs:assert m[metric]==a[field]+b[field],metric
 assert not d['generator_lag']['exceeded_one_period']
 if prefix.startswith('short-fairness-quantum-'):assert d['fanout_windows']=={'completed':96,'offered':96}
 if prefix.endswith('sampling-check'):assert d['latency_sampling']['observations_per_user']==8 and d['latency_sampling']['total_private_observations']==12800 and d['latency_sampling']['all_user_p99_equal_observed_max']
a=(root/'source/server-1024.rs').read_text();b=(root/'source/server-512.rs').read_text();assert a.replace('const FANOUT_QUANTUM: usize = 1024;','const FANOUT_QUANTUM: usize = 512;',1)==b
print('Verified six accepted trials, decoded samples/quantiles, exact protocol counters, resource retirement, matched source change, and reporting metadata.')
