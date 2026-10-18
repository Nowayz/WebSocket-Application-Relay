"""Offline audit of the notification-lock experiment and rejected source patches."""
import csv,gzip,hashlib,io,json,subprocess,tempfile
from pathlib import Path
root=Path(__file__).resolve().parent
for name,h in json.loads((root/'manifest.json').read_text()).items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
def decoded(name):return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())
summary=json.loads((root/'summary.json').read_text())
pairs=[('war_accepted_total','joins'),('war_joined_total','joins'),('war_disconnected_total','normal_closes'),('war_incoming_messages_total','sent_messages'),('war_incoming_bytes_total','sent_bytes'),('war_deliveries_total','deliveries'),('war_delivered_bytes_total','delivered_bytes'),('war_routing_reused_bytes_total','value_bytes')]
failed=[]
for record in summary['runs']:
 name=record['name'];d=json.loads(decoded(name+'.json'));assert d['passed'] and d['exit_codes']==[0,0,0]
 assert d['fanout_windows']=={'completed':96,'offered':96}
 data=decoded(name+'.csv');assert hashlib.sha256(data).hexdigest()==d['csv_sha256']
 rows=list(csv.DictReader(io.StringIO(data.decode())));assert len(rows)==len({r['id'] for r in rows})==100000;assert all(int(r['samples'])==24 for r in rows)
 assert all(float(r['p99_ms'])==float(r['max_ms']) for r in rows)
 for col,stats in d['latency'].items():
  values=sorted(float(r[col]) for r in rows)
  for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:assert values[(len(values)-1)*q//100]==stats[label]
 if 'fanout_csv_sha256' in d:
  data=decoded(name+'-fanout.csv');assert hashlib.sha256(data).hexdigest()==d['fanout_csv_sha256']
  audience=list(csv.DictReader(io.StringIO(data.decode())));assert len(audience)==len({r['id'] for r in audience})==8192;assert all(int(r['samples'])==384 for r in audience)
 assert hashlib.sha256(decoded(name+'.windows.json')).hexdigest()==d['latency_windows_sha256']
 assert not d['generator_lag']['exceeded_one_period']
 m=d['final_metrics'];assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']);assert m['war_channels']==1
 a=d['events'][-1]['counts'];b=d['sidecar_events'][-1]['details']['counts']
 for metric,field in pairs:assert m[metric]==a[field]+b[field],metric
 if 'after_metrics' in d:
  key='war_diagnostic_pending_contentions_total';assert d['ready_metrics'][key]==0
  value=d['after_metrics'][key]-d['ready_metrics'][key];assert value==record['failed_locks_ready_to_measured'];failed.append(value)
assert failed==[49441,258]
with tempfile.TemporaryDirectory(prefix='pending-lock-replay-') as temp:
 tree=Path(temp)
 for file in ['src/server.rs','src/metrics.rs','src/server/socket.rs']:
  p=tree/file;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(subprocess.check_output(['git','show',summary['source_revision']+':'+file]))
 for patch in ['handoff.patch','diagnostic.patch']:subprocess.run(['git','apply',str(root/'source'/patch)],cwd=tree,check=True)
print('Verified six complete trials, decoded samples/quantiles, exact counters/resource retirement, 49,441 vs 258 diagnostic collisions and clean source patch replay.')
