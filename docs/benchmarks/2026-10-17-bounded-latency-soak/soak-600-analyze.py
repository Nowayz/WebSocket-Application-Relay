from pathlib import Path
import csv,json,math,statistics
run=json.loads(Path('artifacts/soak-600-bounded.json').read_text())
assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
rows=list(csv.DictReader(Path('artifacts/soak-600-bounded-users.csv').open()))
args=run['args'];active_rooms=((args['users']//args['room_size'])*args['active_percent']+99)//100
assert len(rows)==active_rooms*args['room_size'] and len({r['id'] for r in rows})==len(rows)
expected=args['seconds']*args['messages_per_second']
for row in rows:
 assert int(row['samples'])==expected
 for field in ['p50_upper_ms','p99_upper_ms','max_ms','schedule_p99_upper_ms','schedule_max_ms']:
  value=float(row[field]);assert math.isfinite(value) and value>=0
 assert float(row['schedule_p99_upper_ms'])>=float(row['p99_upper_ms'])
 assert float(row['schedule_max_ms'])>=float(row['max_ms'])
assert sum(int(r['samples']) for r in rows)==run['measurement']['round_trip']['samples']
assert max(float(r['max_ms']) for r in rows)==run['measurement']['round_trip']['max_ms']
def distribution(field):
 values=sorted(float(r[field]) for r in rows)
 return {name:values[(len(values)*percent+99)//100-1] for name,percent in [('median',50),('p95',95),('p99',99),('maximum',100)]}
series=run['management_samples'];assert series and all(s['sessions']==args['users'] for s in series)
last=run['events'][-1]
resources=['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_variable_bytes']
assert all(last['metrics'][key]==0 for key in resources)
summary={'scope':'Finite ten-minute same-host stability acceptance with bounded per-user histograms. User p99 statistics are power-of-two bucket upper bounds, not exact percentiles. Maxima are exact observed elapsed durations. New client hash; not an old/new relay speedup comparison. Process RSS excludes kernel socket memory.','passed':True,'users':len(rows),'samples_each':expected,'verified_packet_samples':sum(int(r['samples']) for r in rows),'user_p99_upper_ms':distribution('p99_upper_ms'),'user_intended_p99_upper_ms':distribution('schedule_p99_upper_ms'),'worst_packet_ms':max(float(r['max_ms']) for r in rows),'worst_intended_packet_ms':max(float(r['schedule_max_ms']) for r in rows),'pings_observed':sum(int(r['pings']) for r in rows),'per_user_histogram_value_bytes':run['events'][1]['details']['per_user_histogram_value_bytes'],'measurement':run['measurement'],'rss':{},'final_resources':{key:last['metrics'][key] for key in resources},'final_fds':last['server']['fd_count'],'baseline_fds':run['baseline']['server']['fd_count'],'final_established':last['established_server_sockets']}
for role in ['server','client']:
 values=[s[role+'_rss_kib']/1024 for s in series];tail=values[len(values)//2:]
 summary['rss'][role]={'minimum_mib':min(values),'maximum_mib':max(values),'last_mib':values[-1],'second_half_minimum_mib':min(tail),'second_half_maximum_mib':max(tail),'second_half_last_minus_first_mib':tail[-1]-tail[0]}
Path('artifacts/soak-600-analysis.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
