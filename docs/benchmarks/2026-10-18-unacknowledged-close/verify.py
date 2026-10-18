"""Offline audit of withheld policy-Close acknowledgements and admission reuse."""
import gzip,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda b:hashlib.sha256(b).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
p=json.loads((root/'provenance.json').read_text());source=(root/'join-pressure.py').read_bytes()
assert source==subprocess.check_output(['git','show',p['code_revision']+':scripts/join-pressure.py'])
total_healthy=total_failures=total_refusals=0
for name in p['cases']:
 d=json.loads(gzip.decompress((root/'raw'/(name+'.json.gz')).read_bytes()));assert d['passed'] and d['server_exit_code']==0 and not d.get('server_forced_kill')
 assert d['script_sha256']==sha(source)
 for field,file in [('wire_sha256','scripts/input-corpus.py'),('fragment_wire_sha256','scripts/fragment-fairness.py')]:assert d[field]==sha(subprocess.check_output(['git','show',p['code_revision']+':'+file]))
 assert d['server_sha256']=='3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
 m=d['final_metrics'];n=d['connections'];waves=d['waves'];withheld=d['withhold_close_ack'];total_healthy+=d['healthy_verified']
 assert d['healthy_verified']>0 and m['war_channels']==1
 for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:assert m[key]==0,key
 for key,value in d['exact_checks'].items():assert m[key]==value,key
 assert m['war_close_failures_total']==(n*waves if withheld else 0)
 assert m['war_rejected_total']==d['verified_closing_refusals']==(waves if withheld else 0)
 assert m['war_delivered_bytes_total']==d['verified_application_bytes'] and m['war_routing_reused_bytes_total']==(d['healthy_verified']+waves)*66
 for wave in range(1,waves+1):
  retired=next(e for e in d['events'] if e['event']==f'wave-{wave}-retired')
  assert retired['metrics']['war_connections']==retired['metrics']['war_sessions']==1
  if d['ping']:assert retired['verified_pong_peers']==n
  if withheld:
   closing=next(e for e in d['events'] if e['event']==f'wave-{wave}-closing')
   assert closing['metrics']['war_connections']==n+1 and closing['metrics']['war_sessions']==1
   assert closing['metrics']['war_close_failures_total']==(wave-1)*n
   assert retired['metrics']['war_close_failures_total']==wave*n and retired['metrics']['war_rejected_total']==wave
   assert .9<=retired['first_close_to_eof_seconds']<=retired['last_close_to_eof_seconds']<2
 total_failures+=m['war_close_failures_total'];total_refusals+=m['war_rejected_total']
assert (total_healthy,total_failures,total_refusals)==(1547621,160,3)
print('Verified three accepted cases, held admission and refused excess sockets, 160 deadline retirements, acknowledged control, exact byte/counter/resource gates and 1,547,621 healthy echoes.')
