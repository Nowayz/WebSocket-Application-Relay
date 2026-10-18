"""Offline audit of finite HTTP/prejoin deadline tests and exact source snapshots."""
import gzip,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda b:hashlib.sha256(b).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
p=json.loads((root/'provenance.json').read_text());revision=p['code_revision']
def load(name):return json.loads(gzip.decompress((root/'raw'/(name+'.json.gz')).read_bytes()))
def git(file):return subprocess.check_output(['git','show',revision+':'+file])
expected_sha='3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
zero=['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total']
for name in ['short-handshake-idle','short-handshake-drip']:
 d=load(name);assert d['passed'] and d['server_sha256']==expected_sha and d['client_exit_code']==d['server_exit_code']==0
 assert not d.get('client_forced_kill') and not d.get('server_forced_kill')
 assert d['source'].encode()==git('scripts/handshake-pressure.py')
 m=d['final']['metrics'];assert all(m[k]==0 for k in zero) and m['war_channels']==1
 for k,v in d['exact_checks'].items():assert m[k]==v,k
 assert d['args']['connections']==1024 and 4.85<=d['expiry_seconds']['first']<=d['expiry_seconds']['last']<10
 assert d['healthy']['healthy']['verified']==d['healthy']['messages']>0
 assert d['sent_drip_bytes']==(16384 if name.endswith('drip') else 0)
for name,source in p['join_sources'].items():
 d=load(name);assert d['server_sha256']==expected_sha
 assert d['script_sha256']==sha((root/'source'/source).read_bytes())
 assert d['wire_sha256']==sha(git('scripts/input-corpus.py'))
 if 'fragment_wire_sha256' in d:assert d['fragment_wire_sha256']==sha(git('scripts/fragment-fairness.py'))
 if name=='short-join-idle':assert not d['passed'] and d.get('error');continue
 assert d['passed'] and d['server_exit_code']==0 and not d.get('server_forced_kill') and d['healthy_verified']>0
 m=d['final_metrics'];assert all(m[k]==0 for k in zero) and m['war_channels']==1
 for k,v in d['exact_checks'].items():assert m[k]==v,k
 assert m['war_join_rejected_total']==d['connections']*d['waves']
 retired=[e for e in d['events'] if e['event'].endswith('retired')];assert len(retired)==d['waves']
 assert all(1.9<=e['first_close_seconds']<=e['last_close_seconds']<5 for e in retired)
 if d['ping']:assert all(e['verified_pong_peers']==d['connections'] and e['verified_pongs']>=d['connections'] for e in retired)
 if source=='join-pressure.py':
  assert m['war_delivered_bytes_total']==d['verified_application_bytes']
  assert m['war_routing_reused_bytes_total']==(d['healthy_verified']+d['waves'])*66
assert (root/'source/join-pressure.py').read_bytes()==git('scripts/join-pressure.py')
print('Verified two HTTP waves, silent/Ping-active join waves, exact counters/resource retirement, distinct-peer Pong coverage, healthy traffic and all fixture-source provenance; initial overstrict failure retained.')
