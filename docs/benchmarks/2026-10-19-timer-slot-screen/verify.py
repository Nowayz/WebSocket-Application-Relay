"""Audit the archived timer-slot rejection without starting services."""
import argparse,csv,gzip,hashlib,json,statistics
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--check-local-csv',action='store_true');args=parser.parse_args()
p=Path(__file__).resolve().parent
read=lambda name:json.loads((p/name).read_text())
sha=lambda body:hashlib.sha256(body).hexdigest()
for name,digest in read('sha256.json').items():assert sha((p/name).read_bytes())==digest,name
component=read('component-results.json');mixed=read('mixed-results.json')
assert component['passed'] and mixed['passed']
order=['control-1','candidate-1','candidate-2','control-2']
assert [r['label'] for r in component['runs']]==[r['label'] for r in mixed['runs']]==order
for kind,server_name,test_name in [('control','base-server.rs.gz','measurement-tests.rs.gz'),('candidate','candidate-server.rs.gz','measurement-tests.rs.gz')]:
 build=read('component-'+kind+'-build.json')
 assert build['cargo_artifact']['fresh'] is False
 assert build['sources']['src/server.rs']==sha(gzip.decompress((p/server_name).read_bytes()))
 assert build['sources']['src/server/fairness_tests.rs']==sha(gzip.decompress((p/test_name).read_bytes()))
 for row in component['runs']:
  if row['kind']!=kind:continue
  assert row['binary_sha256']==build['binary_sha256']
  m=row['measurement'];assert m['connections']==1024 and m['iterations_per_batch']==100
  assert len(m['ns_per_scheduled_timer'])==7
  assert row['median_ns_per_timer']==statistics.median(m['ns_per_scheduled_timer'])
control=read('native-control-build.json');candidate=read('native-candidate-build.json')
assert control['passed'] and candidate['passed']
assert {key for key in control['source_sha256'].keys()|candidate['source_sha256'].keys() if key.startswith('src/') and control['source_sha256'].get(key)!=candidate['source_sha256'].get(key)}=={'src/server.rs','src/server/fairness_tests.rs'}
assert candidate['source_sha256']['src/server.rs']==sha(gzip.decompress((p/'candidate-server.rs.gz').read_bytes()))
assert candidate['source_sha256']['src/server/fairness_tests.rs']==sha(gzip.decompress((p/'candidate-tests.rs.gz').read_bytes()))
for row in mixed['runs']:
 file=p/('mixed-'+row['label']+'.json');x=json.loads(file.read_text())
 assert sha(file.read_bytes())==row['receipt_sha256']
 assert x['passed'] and x['correctness_passed'] and x['exit_codes']==[0,0,0]
 assert x['sha256']['server']==(control if row['kind']=='control' else candidate)['binary_sha256']
 assert x['fanout_windows']=={'completed':32,'offered':32} and x['fanout_offered_load_met']
 assert x['latency_sampling']['users']==100000 and x['latency_sampling']['observations_per_user']==8
 assert x['latency_sampling']['total_private_observations']==800000
 assert x['latency_sampling']['all_user_p99_equal_observed_max']
 assert x['latency']['p99_ms']==x['latency']['max_ms']
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:
  assert x['final_metrics'][key]==0,(row['label'],key)
 assert x['final_metrics']['war_channels']==1
 if args.check_local_csv:
  for command,flag,key,expected_users in [(x['client_command'],'--user-latency-output','csv_sha256',100000),(x['sidecar_command'],'--fanout-latency-output','fanout_csv_sha256',8192)]:
   path=Path(command[command.index(flag)+1]);assert sha(path.read_bytes())==x[key]
   with path.open() as stream:rows=list(csv.DictReader(stream))
   assert len(rows)==expected_users and len({r['id'] for r in rows})==expected_users
assert '134 passed; 0 failed' in (p/'tests.log').read_text()
assert 'Finished' in (p/'clippy.log').read_text()
assert (p/'fmt.log').read_text()==''
print('PASS: component ABBA, four 100k mixed runs, hashes, counters, offered windows and retirement; runtime candidate rejected')
