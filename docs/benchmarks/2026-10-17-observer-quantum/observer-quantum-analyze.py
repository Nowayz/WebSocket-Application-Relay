from pathlib import Path
import json,hashlib
result={'scope':'Finite same-host comparisons. Fanout/healthy/churn quantiles are histogram upper bounds;maxima exact. Normal100k user guard uses exact client samples. Mixed CPU includes all competing operations,not an isolated forwarding cost.','fanout_pairs':[],'fanout_verified_deliveries':0,'fanout_private_deliveries':0,'fanout_churn_cycles':0,'capacity_pairs':[],'capacity_verified_deliveries':0}
summary=json.loads(Path('artifacts/observer-quantum-summary.json').read_text());assert summary['passed'] and len(summary['runs'])==8
for r in summary['runs']:
 path=Path(r['trace']);assert hashlib.sha256(path.read_bytes()).hexdigest()==r['trace_sha256']
 x=json.loads(path.read_text());assert x['passed'] and x['server_exit_code']==x['client_exit_code']==0 and not x['server_forced_kill']
 m=x['mixed'];fan=m['fanout']['verified'];private=m['healthy']['verified'];churn=m['churn']['verified'];counts=m['counts']
 assert fan%512==0
 assert counts['deliveries']==fan+private+2*churn+1604
 assert counts['sent_messages']==fan//512+private+churn+1025
 assert counts['value_bytes']==1027*(fan//512+private)+9*churn
 assert len(m['churn_lifecycles_per_worker'])==8 and sum(m['churn_lifecycles_per_worker'])==churn
 final=x['events'][-1];assert final['event']=='done'
 for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']:assert final['metrics'][key]==0
 result['fanout_verified_deliveries']+=fan;result['fanout_private_deliveries']+=private;result['fanout_churn_cycles']+=churn
for workers in [1,4]:
 for rep in [1,2]:
  b=next(r['mixed'] for r in summary['runs'] if r['workers']==workers and r['repeat']==rep and r['kind']=='baseline')
  c=next(r['mixed'] for r in summary['runs'] if r['workers']==workers and r['repeat']==rep and r['kind']=='candidate')
  pair={'workers':workers,'repeat':rep,'metrics':{}}
  for group in ['healthy','churn','fanout','inspection']:
   pair['metrics'][group]={}
   for field in ['p99_upper_ms','max_ms','per_second']:
    v,w=b[group][field],c[group][field];pair['metrics'][group][field]={'baseline':v,'candidate':w,'change_percent':100*(w/v-1) if v else None}
  result['fanout_pairs'].append(pair)
summary=json.loads(Path('artifacts/observer-quantum-400k-summary.json').read_text());assert summary['passed'] and len(summary['runs'])==4
for r in summary['runs']:
 p=Path(r['trace']);assert hashlib.sha256(p.read_bytes()).hexdigest()==r['trace_sha256']
 x=json.loads(p.read_text());assert x['passed'] and x['client_exit_code']==x['server_exit_code']==0
 final=x['events'][-1];assert final['server']['fd_count']==x['baseline']['server']['fd_count'] and final['established_server_sockets']==0
 for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']:assert final['metrics'][key]==0
 result['capacity_verified_deliveries']+=r['measurement']['application_deliveries']
for rep in [1,2]:
 b=next(r for r in summary['runs'] if r['kind']=='baseline' and r['repetition']==rep);c=next(r for r in summary['runs'] if r['kind']=='candidate' and r['repetition']==rep)
 p={'repeat':rep,'metrics':{}}
 for key in ['median_user_p99_ms','p99_user_p99_ms','worst_user_p99_ms','worst_packet_ms']:
  v,w=b[key],c[key];p['metrics'][key]={'baseline':v,'candidate':w,'change_percent':100*(w/v-1)}
 for role in ['server','client']:
  v,w=[r['measurement'][role+'_cpu_seconds']*1e9/r['measurement']['application_deliveries'] for r in [b,c]]
  p['metrics'][role+'_cpu_ns_per_message']={'baseline':v,'candidate':w,'change_percent':100*(w/v-1)}
 result['capacity_pairs'].append(p)
assert result['capacity_verified_deliveries']==48000000
Path('artifacts/observer-quantum-analysis.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
