import hashlib,json,statistics
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def read(name):return json.loads((ROOT/'artifacts'/name).read_text())
def summarize(name):
 d=read(name);assert d['passed'];out={}
 for mode in ['fanout','private']:
  for kind in ['cpp','rust','control','modern']:
   rows=[r for r in d['cases'] if r['case']==mode and r['kind']==kind];a='fanout' if mode=='fanout' else 'healthy'
   assert rows and all(r['passed'] for r in rows)
   for r in rows:
    trace=ROOT/r['trace'];assert hashlib.sha256(trace.read_bytes()).hexdigest()==r['trace_sha256']
    full=json.loads(trace.read_text());assert full['passed']
    if kind in ['rust','control']:
     assert all(e['metrics']['war_routing_copied_bytes_total']==0 for e in full['events'])
   vals={
    'rate':[r['mixed'][a]['per_second'] for r in rows],
    'cpu_us':[r['mixed']['server_cpu_seconds']*1e6/r['mixed'][a]['verified'] for r in rows],
    'p99_upper_ms':[r['mixed'][a]['p99_upper_ms'] for r in rows],
    'rss_mib':[r['sampled_server_rss_kib'][1]/1024 for r in rows]}
   out[mode+'/'+kind]={'trials':len(rows),**{k:{'median':statistics.median(v),'min':min(v),'max':max(v),'all':v} for k,v in vals.items()}}
 return out
out={'prototype':summarize('rust-owner-fanout-comparison.json'),'final':summarize('rust-owner-fanout-all-comparison.json')}
mixed=read('rust-owner-fanout-all-mixed.json');assert mixed['passed'] and len(mixed['runs'])==6
out['mixed']=[{k:r[k] for k in ['kind','repeat','result','server_cpu_seconds','rss_kib']} for r in mixed['runs']]
capacity=read('rust-owner-fanout-all-capacity.json');assert capacity['passed'] and len(capacity['runs'])==4
out['capacity']=[]
for r in capacity['runs']:
 assert r['passed'] and r['users']==100000 and r['samples_each']==120
 assert r['after']['fd_count']==r['baseline']['fd_count']
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
  assert r['final_metrics'][key]==0,(r['kind'],key,r['final_metrics'][key])
 out['capacity'].append({k:r[k] for k in ['kind','repeat','users','samples_each','population_rss_kib','server_cpu_seconds','p99_ms','max_ms','schedule_p99_ms','csv','csv_sha256']})
profile=read('rust-fanout-profile-v2.json');assert profile['passed']
out['passed']=True
(ROOT/'artifacts/rust-owner-fanout-summary.json').write_text(json.dumps(out,indent=2)+'\n')
for stage in ['prototype','final']:
 for k,v in out[stage].items():print(stage,k,*(f'{metric}={v[metric]["median"]:.6f}' for metric in ['rate','cpu_us','p99_upper_ms']))
for r in out['capacity']:print('capacity',r['kind'],r['repeat'],'RSS',r['population_rss_kib']/1024,'CPU',r['server_cpu_seconds'],'user p99',r['p99_ms'])
