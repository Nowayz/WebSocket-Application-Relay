import json,statistics,hashlib,csv
from pathlib import Path
rows=[]
for kind in ['before','after','cpp']:
 for repeat in [1,2]:
  path=Path(f'artifacts/mixed100k-final-{kind}-{repeat}.json')
  r=json.loads(path.read_text());assert r['passed'] and all(code==0 for code in r['exit_codes'])
  measured=next(e['details'] for e in r['events'] if e['event']=='measured')
  other=next(e['details'] for e in r['sidecar_events'] if e['event']=='measured')
  assert r['fanout_windows']['offered']==480
  assert 0 < r['fanout_windows']['completed'] <= 480
  assert measured['round_trip']['samples']==12000000
  assert other['fanout']['verified']==r['fanout_windows']['completed']*8192*4
  assert other['churn']['verified']==960
  csvpath=path.with_suffix('.csv');users=list(csv.DictReader(csvpath.open()))
  assert len(users)==100000 and len({row['id'] for row in users})==100000
  assert all(int(row['samples'])==120 for row in users)
  assert hashlib.sha256(csvpath.read_bytes()).hexdigest()==r['csv_sha256']
  for column,stats in r['latency'].items():
   values=sorted(float(row[column]) for row in users)
   for name,p in [('median',50),('p95',95),('p99',99),('max',100)]:assert values[(len(values)-1)*p//100]==stats[name]
  start=next(e['controller_monotonic_seconds'] for e in r['sidecar_events'] if e['event']=='measured')
  finish=next(e['controller_monotonic_seconds'] for e in r['sidecar_events'] if e['event']=='done')
  item={'kind':kind,'repeat':repeat,'csv_sha256':r['csv_sha256'],'server_sha256':r['sha256']['server'],
        'cpu_seconds':r['server_cpu_seconds'],'latency':r['latency'],
        'fanout_windows':r['fanout_windows'],'missed_windows':480-r['fanout_windows']['completed'],'fanout':other['fanout'],'churn':other['churn'],'inspection':other['inspection'],
        'peak_sampled_rss_mib':max(e['process']['rss_kib'] for e in r['monitoring'])/1024,
        'peak_sampled_queue_mib':max(e['metrics']['war_queued_bytes'] for e in r['monitoring'])/1024**2,
        'users_with_p99_over_2ms':sum(float(row['p99_ms'])>2 for row in users),
        'users_with_p99_over_5ms':sum(float(row['p99_ms'])>5 for row in users),
        'sidecar_cleanup_seconds':finish-start,'private_round_trip':measured['round_trip']}
  rows.append(item)
summary={'scope':'Two rotated 30-second runs per runtime.100000users,120samples/user/run.400000private probes/s+524288room fanout deliveries/s+32churn lifecycles/s+200offered inspections/s. Same physically separated CPU cores, checked source/body/FIFO, exact counters and complete cleanup.','runs':rows,'medians':{}}
for kind in ['before','after','cpp']:
 runs=[r for r in rows if r['kind']==kind]
 summary['medians'][kind]={
  **{key:statistics.median(r[key] for r in runs) for key in ['cpu_seconds','peak_sampled_rss_mib','peak_sampled_queue_mib','users_with_p99_over_2ms','users_with_p99_over_5ms','sidecar_cleanup_seconds','missed_windows']},
  'median_user_p99_ms':statistics.median(r['latency']['p99_ms']['median'] for r in runs),
  'p99_user_p99_ms':statistics.median(r['latency']['p99_ms']['p99'] for r in runs),
  'worst_user_p99_ms':statistics.median(r['latency']['p99_ms']['max'] for r in runs),
  'worst_probe_ms':max(r['latency']['max_ms']['max'] for r in runs),
  'fanout_max_ms':max(r['fanout']['max_ms'] for r in runs),
  'fanout_p99_upper_ms':[r['fanout']['p99_upper_ms'] for r in runs],
  'private_p99_upper_ms':[r['private_round_trip']['p99_upper_ms'] for r in runs]}
Path('artifacts/mixed100k-final-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary['medians'],indent=2))
