"""Offline integrity and semantic checks for the short bridge memory study."""
import hashlib,json
from pathlib import Path
root=Path(__file__).resolve().parent
manifest=json.loads((root/'manifest.json').read_text())
for p,h in manifest.items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
def load(name):return json.loads((root/'artifacts'/name).read_text())
for name in ['bridge-buffer-final-memory.json','bridge-buffer-matched-control.json']+[f'bridge-buffer-memory-{i}-{label}.json' for i,label in enumerate(['control','candidate','candidate','control'])]:
 d=load(name);assert d['passed'] and d['verified_probes']==256*144 and d['exit_codes']==[0,0]
 m=d['retired']['metrics'];assert m['war_incoming_messages_total']==256*144 and m['war_deliveries_total']==256*145
 assert m['war_routing_copied_bytes_total']==0
 assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes'])
a=load('bridge-buffer-matched-control.json');b=load('bridge-buffer-final-memory.json')
assert a['joined']['bridge']['RssAnon']-b['joined']['bridge']['RssAnon']>30000
build=json.loads((root/'build.json').read_text());assert b['bridge_sha256']==build['binary_sha256']
assert hashlib.sha256((root/'legacy_bridge.rs').read_bytes()).hexdigest()==build['source_sha256']
for name in ['bridge-buffer-final-large.json','bridge-buffer-js-check.json','bridge-buffer-nagle.json','bridge-buffer-adoption.json']:assert load(name)['passed'],name
assert load('bridge-buffer-final-large.json')['cases'][-1]['payload_bytes']+15==16*1024*1024
assert not load('bridge-buffer-baseline-memory.json')['passed']
print(f'Verified {len(manifest)} archived files; exact echo counts, zero-copy routing counter, retirement, memory saving, large frames, JS, delayed ACK, and local adoption receipts passed.')
