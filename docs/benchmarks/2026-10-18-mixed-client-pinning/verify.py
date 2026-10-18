"""Offline verification of optional client affinity and finite mixed-load samples."""
import csv,gzip,hashlib,io,json
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
def raw(name):return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())
summary=json.loads((root/'summary.json').read_text());helper=(root/'source/benchmark.py').read_bytes();assert sha(helper)==summary['shared_helper_sha256']
measured=(root/'source/measured-runner.py').read_text();final=(root/'source/final-runner.py').read_text()
expected=measured.replace("help='Pin both clients I/O workers individually inside their existing CPU masks'","help='Pin client I/O workers individually inside their existing CPU masks'",1).replace("'sidecar': digest(args.sidecar), 'runner': digest(__file__)}","'sidecar': digest(args.sidecar), 'runner': digest(__file__), 'benchmark_helper': digest(runtime.__file__)}",1)
assert expected==final
pairs=[('war_accepted_total','joins'),('war_joined_total','joins'),('war_disconnected_total','normal_closes'),('war_incoming_messages_total','sent_messages'),('war_incoming_bytes_total','sent_bytes'),('war_deliveries_total','deliveries'),('war_delivered_bytes_total','delivered_bytes'),('war_routing_reused_bytes_total','value_bytes')]
for r in summary['runs']:
 name=r['name'];d=json.loads(raw(name+'.json'));assert d['passed'] and d['exit_codes']==[0,0,0]
 smoke=name.endswith('final-check');assert d['sha256']['runner']==sha(final.encode() if smoke else measured.encode())
 if smoke:assert d['sha256']['benchmark_helper']==sha(helper)
 data=raw(name+'.csv');assert sha(data)==d['csv_sha256'];rows=list(csv.DictReader(io.StringIO(data.decode())))
 users=1600 if smoke else 100000;samples=8 if smoke else 24
 assert len(rows)==len({x['id'] for x in rows})==users and all(int(x['samples'])==samples for x in rows)
 assert all(float(x['p99_ms'])==float(x['max_ms']) for x in rows)
 for col,stats in d['latency'].items():
  vals=sorted(float(x[col]) for x in rows)
  for label,q in [('median',50),('p95',95),('p99',99),('max',100)]:assert vals[(len(vals)-1)*q//100]==stats[label]
 if not smoke:
  assert d['fanout_windows']=={'completed':96,'offered':96}
  data=raw(name+'-fanout.csv');assert sha(data)==d['fanout_csv_sha256'];audience=list(csv.DictReader(io.StringIO(data.decode())))
  assert len(audience)==len({x['id'] for x in audience})==8192 and all(int(x['samples'])==384 for x in audience)
 assert sha(raw(name+'.windows.json'))==d['latency_windows_sha256'];assert not d['generator_lag']['exceeded_one_period']
 pins=d['client_thread_affinity'];assert d['args']['pin_client_workers']==r['pinned']
 if r['pinned']:
  assert len(pins)==len({p['tid'] for p in pins})==16
  for group,cpus in [('capacity',[8,9,10,11,24,25,26,27]),('sidecar',[12,13,14,15,28,29,30,31])]:
   workers=[p for p in pins if p['client']==group];assert len(workers)==8 and sorted(p['cpu'] for p in workers)==cpus
   assert all(p['name']=='tokio-rt-worker' and p['method']=='harness' and p['before']==cpus for p in workers)
 else:assert pins==[]
 m=d['final_metrics'];assert m['war_channels']==1
 assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total'])
 a=d['events'][-1]['counts'];b=d['sidecar_events'][-1]['details']['counts']
 for metric,field in pairs:assert m[metric]==a[field]+b[field],metric
 assert r['private_worst_distribution_ms']==d['latency']['max_ms']
 for group in ['capacity','sidecar']:assert r['client_cpu_seconds'][group]==d['client_after'][group]['cpu_seconds']-d['client_before'][group]['cpu_seconds']
print('Verified five accepted cases, measured/final runner provenance, decoded samples/quantiles, sixteen pinned workers per enabled case, counters and resource retirement.')
