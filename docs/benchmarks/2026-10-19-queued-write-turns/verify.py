"""Offline validation of queued-write diagnostic evidence."""
import gzip,hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
build=json.loads((p/'build.json').read_text());assert build['passed']
summary=json.loads((p/'summary.json').read_text())
for case in ['large','small']:
 x=json.loads((p/(case+'.json')).read_text())
 assert x['passed'] and x['client_exit_code']==0 and x['server_exit_code']==0 and not x['server_forced_kill']
 assert x['server_sha256']==build['binary_sha256']
 rows=[json.loads(line.split('WRITE_TURN_PROFILE ',1)[1]) for line in x['server_log'].splitlines() if line.startswith('WRITE_TURN_PROFILE ')]
 assert len(rows)==4 and all(len(r['counts'])==len(r['nanos'])==len(r['max_nanos'])==65 for r in rows)
 counts=[sum(r['counts'][i] for r in rows) for i in range(65)]
 nanos=[sum(r['nanos'][i] for r in rows) for i in range(65)]
 actual={'turns':sum(counts),'zero_byte_turns':counts[0],
         'max_bytes':max(r['max_bytes'] for r in rows),'max_frames':max(r['max_frames'] for r in rows),
         'quantum_exhaustions':sum(counts[19:]),
         'buckets':[{'bytes_lower':0 if i==0 else 2**(i-1),'bytes_upper':0 if i==0 else 2**i-1,
                     'turns':v,'average_us':nanos[i]/v/1000,
                     'maximum_us':max(r['max_nanos'][i] for r in rows)/1000} for i,v in enumerate(counts) if v]}
 assert actual==summary[case] and actual['quantum_exhaustions']==0
 final=x['events'][-1]['metrics']
 for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
  assert final[key]==0,(case,key)
positive=(p/'positive-control.log').read_text()
assert '1 passed; 0 failed' in positive
rows=[json.loads(line.split('WRITE_TURN_PROFILE ',1)[1]) for line in positive.splitlines() if line.startswith('WRITE_TURN_PROFILE ')]
assert any(r['max_bytes']==262144 and r['counts'][19]>0 for r in rows)
for name,digest in json.loads((p/'sha256.json').read_text()).items():
 assert hashlib.sha256((p/name).read_bytes()).hexdigest()==digest,name
print('PASS: two diagnostic fixtures, four worker reports each, zero exhausted turns, positive quantum control and retired resources')
