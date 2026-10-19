import argparse, csv, hashlib, json
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--check-local-csv',action='store_true');args=p.parse_args()
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
x=json.loads((r/'receipt.json').read_text()); b=json.loads((r/'build.json').read_text())
assert x['passed'] and x['correctness_passed'] and b['passed']
assert b['cargo_artifact']['fresh'] is False
assert x['fanout_windows']=={'completed':32,'offered':32}
assert x['exit_codes']==[0,0,0]
assert x['latency_sampling']['users']==100000 and x['latency_sampling']['observations_per_user']==8
for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_native_connection_slots','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
    assert x['final_metrics'][key]==0,key
if args.check_local_csv:
    cmd=x['client_command'];path=Path(cmd[cmd.index('--user-latency-output')+1])
    assert hashlib.sha256(path.read_bytes()).hexdigest()==x['csv_sha256']
    rows=list(csv.DictReader(path.open()));assert len(rows)==100000
    assert all(int(row['samples'])==8 and row['p99_ms']==row['max_ms'] for row in rows)
    assert max(float(row['max_ms']) for row in rows)==x['latency']['max_ms']['max']
print('Short fairness evidence verified: fresh build, 100k users, offered load met and resources retired.')
