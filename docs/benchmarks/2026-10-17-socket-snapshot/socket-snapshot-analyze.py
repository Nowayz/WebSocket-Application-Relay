from pathlib import Path
import hashlib,json
result={'scope':'Matched exact-client finite loopback comparisons. Across-user percentiles of per-user p99, not pooled packet percentiles. CPU normalized by fully verified application messages. Independent fresh processes; two pairs per rate.','pairs':[],'verified_application_messages':0}
for rate in ['400k','600k']:
 summary=json.loads(Path(f'artifacts/socket-snapshot-{rate}-summary.json').read_text());assert summary['passed']
 for row in summary['runs']:
  trace=Path(row['trace']);assert hashlib.sha256(trace.read_bytes()).hexdigest()==row['trace_sha256']
  data=json.loads(trace.read_text());assert data['passed'] and data['client_exit_code']==data['server_exit_code']==0
  final=data['events'][-1];assert final['event']=='done'
  assert final['server']['fd_count']==data['baseline']['server']['fd_count']
  assert final['established_server_sockets']==0
  for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']:
   assert final['metrics'][key]==0,(trace,key)
  result['verified_application_messages']+=row['measurement']['application_deliveries']
 for rep in [1,2]:
  baseline=next(r for r in summary['runs'] if r['kind']=='baseline' and r['repetition']==rep)
  candidate=next(r for r in summary['runs'] if r['kind']=='snapshot' and r['repetition']==rep)
  pair={'rate':rate,'repetition':rep,'metrics':{}}
  for field in ['median_user_p99_ms','p95_user_p99_ms','p99_user_p99_ms','worst_user_p99_ms','worst_packet_ms']:
   b,c=baseline[field],candidate[field]
   pair['metrics'][field]={'baseline':b,'candidate':c,'change_percent':100*(c/b-1)}
  for key in ['server_cpu_seconds','client_cpu_seconds']:
   values=[r['measurement'][key]*1e9/r['measurement']['application_deliveries'] for r in [baseline,candidate]]
   pair['metrics'][key.replace('_seconds','_ns_per_message')]={'baseline':values[0],'candidate':values[1],'change_percent':100*(values[1]/values[0]-1)}
  for field in ['median','p95','p99','maximum']:
   b,c=[r['schedule_p99_ms'][field] for r in [baseline,candidate]]
   pair['metrics']['intended_user_p99_'+field+'_ms']={'baseline':b,'candidate':c,'change_percent':100*(c/b-1)}
  result['pairs'].append(pair)
assert result['verified_application_messages']==120000000
Path('artifacts/socket-snapshot-analysis.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
