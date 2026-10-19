"""Audit diagnostic phase counts and independent wire-byte conservation offline."""
import gzip,hashlib,json,math
from pathlib import Path
p=Path(__file__).resolve().parent
read=lambda name:json.loads((p/name).read_text())
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in read('sha256.json').items():assert sha((p/name).read_bytes())==digest,name
build=read('phased-build.json');assert build['passed'] and build['cargo_artifact']['fresh'] is False
for path in ['src/server.rs','src/server/socket.rs','src/admin.rs']:
 assert sha(gzip.decompress((p/('diagnostic-'+path.replace('/','-')+'.gz')).read_bytes()))==build['source_sha256'][path]
summary=read('summary.json')
for mode in ['private','fanout']:
 x=read('phased-'+mode+'.json')
 assert x['passed'] and x['diagnostic_only'] and x['server_sha256']==build['binary_sha256']
 assert x['client_exit_code']==x['server_exit_code']==0 and not x['server_forced_kill']
 records=[json.loads(line.split('IO_CALL_CENSUS ',1)[1]) for line in x['server_log'].splitlines() if line.startswith('IO_CALL_CENSUS ')]
 assert {r['thread'] for r in records}=={'war-io-0','war-io-1','war-io-2','war-io-3'} and len(records)==4
 assert all(len(r['data']['phases'])==3 for r in records)
 for phase,label in enumerate(['setup','measurement','cleanup']):
  data=[r['data']['phases'][phase] for r in records]
  actual={}
  for i,name in enumerate(['read','control_write','send','send_vectored']):
   row={key:(max if key.startswith('max_') else sum)(d['io'][i][key] for d in data) for key in data[0]['io'][i]}
   assert row['calls']==sum(row[k] for k in ['successes','again','interrupted','errors'])
   assert row['bytes']<=row['requested_bytes'] and row['short']<=row['successes']-row['zero']
   actual[name]=row
  saved=summary[mode]['phases'][label];assert saved['io']==actual
  for kind in ['poll','wake','writable']:
   unit={key:sum(d[kind][key] for d in data) for key in data[0][kind]}
   assert unit==saved['unit'][kind] and unit['calls']==sum(unit[k] for k in ['successes','interrupted','errors'])
  assert saved['poll_events']==sum(d['poll_events'] for d in data)
  assert saved['poll_nonblocking']==sum(d['poll_nonblocking'] for d in data)
 ready=next(e for e in x['events'] if e['event']=='ready');measured=next(e for e in x['events'] if e['event']=='measured')
 assert ready['census_next_phase']==1 and measured['census_next_phase']==2
 phase=summary[mode]['phases']['measurement']
 delta={key:measured['metrics'][key]-ready['metrics'][key] for key in phase['metric_delta']}
 assert delta==phase['metric_delta']
 io=phase['io'];frames=delta['war_deliveries_total'];messages=delta['war_incoming_messages_total']
 sent=io['send']['bytes']+io['send_vectored']['bytes']
 assert sent==delta['war_delivered_bytes_total']+4*frames
 assert io['read']['bytes']==delta['war_incoming_bytes_total']+8*messages
 assert math.isclose(phase['send_calls_per_delivery'],(io['send']['calls']+io['send_vectored']['calls'])/frames)
 assert all(io[k]['short']==io[k]['again']==io[k]['errors']==0 for k in ['send','send_vectored'])
 assert phase['unit']['writable']['calls']==0
 final=x['events'][-1]['metrics']
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:assert final[key]==0,(mode,key)
 if mode=='fanout':assert final['war_deliveries_total']-measured['metrics']['war_deliveries_total']==2048*2049//2
 old=read('lifetime-'+mode+'.json');assert old['passed'] and old['server_sha256']==read('lifetime-build.json')['binary_sha256']
assert read('host-probe.json')['host_policy_changed'] is False
print('PASS: four workers x three phases, exact 4/8-byte header conservation, accounting/resource gates, notification cleanup and frozen identity')
