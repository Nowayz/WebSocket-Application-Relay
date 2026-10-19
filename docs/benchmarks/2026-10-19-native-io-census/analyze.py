import json
from pathlib import Path
out={}
for mode in ['private','fanout']:
 x=json.loads(Path(f'artifacts/io-census-20261019-phased-{mode}.json').read_text())
 assert x['passed'] and x['diagnostic_only'] and x['server_exit_code']==x['client_exit_code']==0
 records=[json.loads(line.split('IO_CALL_CENSUS ',1)[1]) for line in x['server_log'].splitlines() if line.startswith('IO_CALL_CENSUS ')]
 assert len(records)==4 and len({r['thread'] for r in records})==4
 phases=[]
 for phase in range(3):
  data=[r['data']['phases'][phase] for r in records]
  io=[]
  for i in range(4):
   row={key:(max if key.startswith('max_') else sum)(d['io'][i][key] for d in data) for key in data[0]['io'][i]}
   assert row['calls']==sum(row[k] for k in ['successes','again','interrupted','errors'])
   assert row['short']<=row['successes']-row['zero'] and row['bytes']<=row['requested_bytes']
   io.append(row)
  units={kind:{key:sum(d[kind][key] for d in data) for key in data[0][kind]} for kind in ['poll','wake','writable']}
  for row in units.values():assert row['calls']==sum(row[k] for k in ['successes','interrupted','errors'])
  phases.append({'io':dict(zip(['read','control_write','send','send_vectored'],io)),'unit':units,'poll_events':sum(d['poll_events'] for d in data),'poll_nonblocking':sum(d['poll_nonblocking'] for d in data)})
 ready=next(e for e in x['events'] if e['event']=='ready');measured=next(e for e in x['events'] if e['event']=='measured')
 assert ready['census_next_phase']==1 and measured['census_next_phase']==2
 delta={key:measured['metrics'][key]-ready['metrics'][key] for key in ['war_deliveries_total','war_delivered_bytes_total','war_incoming_messages_total','war_incoming_bytes_total']}
 phase=phases[1];sent=phase['io']['send']['bytes']+phase['io']['send_vectored']['bytes'];read=phase['io']['read']['bytes'];frames=delta['war_deliveries_total'];msgs=delta['war_incoming_messages_total']
 assert delta['war_delivered_bytes_total']+2*frames<=sent<=delta['war_delivered_bytes_total']+10*frames,(mode,'send')
 assert delta['war_incoming_bytes_total']+6*msgs<=read<=delta['war_incoming_bytes_total']+14*msgs,(mode,'read',read,delta)
 phase['metric_delta']=delta
 phase['send_calls_per_delivery']=(phase['io']['send']['calls']+phase['io']['send_vectored']['calls'])/frames
 phase['mean_read_bytes_per_positive_call']=read/(phase['io']['read']['successes']-phase['io']['read']['zero'])
 phase['mean_ws_send_header_bytes']=(sent-delta['war_delivered_bytes_total'])/frames
 phase['mean_ws_receive_header_bytes']=(read-delta['war_incoming_bytes_total'])/msgs
 out[mode]={'phases':dict(zip(['setup','measurement','cleanup'],phases))}
 print(mode,json.dumps(phase,indent=2))
Path('artifacts/io-census-20261019-phased-summary.json').write_text(json.dumps(out,indent=2)+'\n')
