from pathlib import Path
import csv,hashlib,json
root=Path.cwd()
result={'scope':'Exact-client matched client placement control;selected relay unchanged. Per-user p99 distributions include relay,kernel and generator reception. Counter diagnostics are separate,not ordinary timing evidence.','runs':[],'pairs':[],'ordinary_verified_messages':0,'diagnostic_verified_messages':0}
for rate in ['400k','600k']:
 summary=json.loads(Path(f'artifacts/client-placement-{rate}-summary.json').read_text());assert summary['passed']
 for row in summary['runs']:
  path=Path(row['trace']);assert hashlib.sha256(path.read_bytes()).hexdigest()==row['trace_sha256']
  run=json.loads(path.read_text());assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
  assert run['server_sha256']=='c9a6c6daf6b6433d4cc4ac16227e61f0d6690ce8f102e2ea95acdb1ea4f6ded0'
  assert run['client_sha256']=='888485f3b416a452ceae006d96eaa38f01696bb49af2e6187d4da3f65c528a82'
  ready,measured=run['events'][1:3];final=run['events'][-1];assert final['event']=='done'
  assert final['server']['fd_count']==run['baseline']['server']['fd_count'] and final['established_server_sockets']==0
  for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']:assert final['metrics'][key]==0
  per_user=list(csv.DictReader(path.with_suffix('.csv').open()));assert len(per_user)==100000 and len({r['id'] for r in per_user})==100000 and all(int(r['samples'])==(120 if rate=='400k' else 180) for r in per_user)
  fixed=[a for a in run['thread_affinity'] if a['pid']==ready['client_pid']]
  assert len(fixed)==(8 if row['kind']=='pinned' else 0)
  if fixed:assert sorted(a['cpu'] for a in fixed)==list(range(8,16)) and all(a['verified_after_measurement'] for a in fixed)
  row=dict(row);row['rate']=rate;row['worker_cpu_seconds']={}
  for role in ['server','client']:
   main=ready[role+'_pid'];values=sorted(v-ready[role]['thread_cpu_seconds'][t] for t,v in measured[role]['thread_cpu_seconds'].items() if int(t)!=main)
   assert len(values)==8
   row['worker_cpu_seconds'][role]={'minimum':min(values),'maximum':max(values),'sum':sum(values)}
  result['runs'].append(row);result['ordinary_verified_messages']+=row['measurement']['application_deliveries']
for rate in ['400k','600k']:
 for rep in [1,2]:
  b=next(r for r in result['runs'] if r['rate']==rate and r['kind']=='baseline' and r['repetition']==rep);c=next(r for r in result['runs'] if r['rate']==rate and r['kind']=='pinned' and r['repetition']==rep)
  pair={'rate':rate,'repetition':rep,'metrics':{}}
  for field in ['median_user_p99_ms','p99_user_p99_ms','worst_user_p99_ms','worst_packet_ms']:
   first,second=b[field],c[field];pair['metrics'][field]={'baseline':first,'pinned':second,'change_percent':100*(second/first-1)}
  for role in ['server','client']:
   first,second=[r['measurement'][role+'_cpu_seconds']*1e9/r['measurement']['application_deliveries'] for r in [b,c]]
   pair['metrics'][role+'_cpu_ns_per_message']={'baseline':first,'pinned':second,'change_percent':100*(second/first-1)}
  result['pairs'].append(pair)
for prefix in ['client-placement-counter','client-placement-pinned-counter']:
 run=json.loads(Path('artifacts/'+prefix+'.json').read_text());assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
 final=run['events'][-1];assert final['server']['fd_count']==run['baseline']['server']['fd_count'] and final['established_server_sockets']==0
 for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']:assert final['metrics'][key]==0
 counters=json.loads(Path('artifacts/'+prefix+'-analysis.json').read_text())
 groups={}
 for role,name in [('server','war-io-'),('client','tokio-rt-worker')]:
  threads=[t for t in counters['threads'] if t['role']==role and (t['name'].startswith(name) if role=='server' else t['name']==name)];assert len(threads)==8
  groups[role]={'migrations':sum(t['events']['cpu-migrations']['value'] for t in threads),'migrations_per_worker':[t['events']['cpu-migrations']['value'] for t in threads],'task_clock_seconds':sum(t['events']['task-clock']['value'] for t in threads)/1000}
 if 'pinned-counter' in prefix:assert max(groups['client']['migrations_per_worker'])<=1
 assert groups['server']['migrations']==0
 result[prefix]={'worker_groups':groups,'cpu_accounting':counters['cpu_accounting']};result['diagnostic_verified_messages']+=run['measurement']['application_deliveries']
assert result['ordinary_verified_messages']==120000000 and result['diagnostic_verified_messages']==36000000
Path('artifacts/client-placement-analysis.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'pairs':result['pairs'],'counters':{k:v for k,v in result.items() if k.startswith('client-placement-')}},indent=2))
