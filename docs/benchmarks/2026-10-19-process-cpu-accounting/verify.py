"""Audit CPU parser evidence and raw user/system traffic deltas offline."""
import gzip,hashlib,json,math
from pathlib import Path
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
read=lambda name:json.loads((p/name).read_text())
for name,digest in read('sha256.json').items():assert sha((p/name).read_bytes())==digest,name
provenance=read('provenance.json');assert provenance['clock_ticks_per_second']==100
for path,hashes in provenance['files'].items():
 for kind in ['base','selected']:
  assert sha(gzip.decompress((p/(kind+'-'+Path(path).name+'.gz')).read_bytes()))==hashes[kind+'_sha256']
live=read('live.json');assert live['passed'] and live['exit_code']==0 and live['comm']=='relay ) probe'
assert live['original_snapshot']['cpu_seconds']==0
assert live['selected_snapshot']['cpu_seconds']==live['mixed_snapshot']['cpu_seconds']==0.16
for sample in [live['selected_snapshot'],live['mixed_snapshot']]:
 assert math.isclose(sample['cpu_seconds'],sample['user_cpu_seconds']+sample['system_cpu_seconds'])
 for key in ['user_cpu_seconds','system_cpu_seconds']:assert abs(sample[key]-live['child_os_times'][key])<=2/live['clock_ticks_per_second']
summary=read('runs.json');assert summary['passed'] and [r['mode'] for r in summary['runs']]==['private','fanout']
for row in summary['runs']:
 x=read(row['mode']+'.json');assert x['passed'] and not x['profile_diagnostic'] and x['server_exit_code']==x['client_exit_code']==0 and not x['server_forced_kill']
 ready=next(e['process'] for e in x['events'] if e['event']=='ready')
 measured=next(e['process'] for e in x['events'] if e['event']=='measured')
 times={key:measured[key]-ready[key] for key in ['cpu_seconds','user_cpu_seconds','system_cpu_seconds']}
 assert times==row['cpu'] and all(v>=0 for v in times.values())
 assert math.isclose(times['cpu_seconds'],times['user_cpu_seconds']+times['system_cpu_seconds'])
 assert math.isclose(row['system_share_percent'],100*times['system_cpu_seconds']/times['cpu_seconds'])
 assert math.isclose(x['mixed']['server_cpu_seconds'],times['cpu_seconds'])
 final=x['events'][-1]['metrics']
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:assert final[key]==0,(row['mode'],key)
for name,count in [('tests.log',2),('snapshot-checks.log',6),('inspection-cleanup.log',1),('mixed-cleanup.log',2)]:
 log=(p/name).read_text();assert f'Ran {count} test' in log and '\nOK' in log
print('PASS: command-name parser fix, split tick accounting, real child, two ordinary traffic receipts and focused compatibility checks')
