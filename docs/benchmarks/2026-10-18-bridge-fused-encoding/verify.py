"""Offline audit of single-buffer legacy binary encoding and local adoption."""
import hashlib,json,re,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda b:hashlib.sha256(b).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
def load(name):return json.loads((root/'raw'/name).read_text())
build=json.loads((root/'build.json').read_text());source=root/'source'
selected=(source/'selected-legacy_bridge.rs').read_text();direct=(source/'measured-direct-legacy_bridge.rs').read_text();control=(source/'control-legacy_bridge.rs').read_text()
assert control.encode()==subprocess.check_output(['git','show','3737ced:src/legacy_bridge.rs'])
assert selected.encode()==subprocess.check_output(['git','show',build['source_revision']+':src/legacy_bridge.rs'])
assert sha(selected.encode())==build['source_sha256']
runtime=selected.split('\n#[cfg(test)]')[0].replace('                // Write the envelope and value once, directly into their final storage.\n','')
assert runtime.rstrip()==direct.rstrip()
a=control.index('            if target != 0 {');b=control.index('            Some(match legacy::command',a)
x=direct.index('            if target != 0 {');y=direct.index('            Some(match legacy::command',x)
assert control[:a]==direct[:x] and control[b:]==direct[y:]
variants=['control','direct','direct','control'];cases=[]
for i,label in enumerate(variants):
 rows=[json.loads(line) for line in (root/'raw'/f'bridge-fused-screen-valid-{i}-{label}.jsonl').read_text().splitlines()]
 assert [r['payload_bytes'] for r in rows]==[1,16,1024,65536]
 assert all(len(r['ns_per_operation'])==9 and min(r['ns_per_operation'])>0 for r in rows);cases.append(rows)
rows=cases[0];calls=sum(1+1000+9*r['operations_per_batch'] for r in rows)
assert calls==2722436
counts=[]
for label in ['control','direct']:
 header=(root/'raw'/f'heap-{label}.heap').read_text().splitlines()[0]
 m=re.search(r'\[(\d+):\s*(\d+)\]',header);assert m;counts.append(tuple(map(int,m.groups())))
removed=tuple(a-b for a,b in zip(*counts));assert removed==(calls,2216105299)
def width(n):return 1 if n<24 else 2 if n<256 else 3 if n<65536 else 5 if n<2**32 else 9
assert removed[1]==sum((r['payload_bytes']+width(r['payload_bytes']))*(1+1000+9*r['operations_per_batch']) for r in rows)
for name in [f'bridge-fused-wire-{i}-{l}.json' for i,l in enumerate(variants)]+['bridge-fused-wire-64k.json','bridge-fused-wire-1m.json','bridge-fused-selected-wire-1m.json']:
 d=load(name);assert d['passed'] and d['exit_codes']==[0,0]
 n=d['clients'];rounds=d['rounds_per_client'];m=d['retired']['metrics'];assert d['verified_probes']==n*rounds
 assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==n
 assert m['war_incoming_messages_total']==n*rounds and m['war_deliveries_total']==n*(rounds+1)
 assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']) and m['war_channels']==1
 if name.startswith('bridge-fused-selected'):assert d['bridge_sha256']==build['binary_sha256']
for name in ['bridge-fused-js-check.json','bridge-fused-selected-js-check.json','bridge-fused-nagle.json','bridge-fused-selected-nagle.json','bridge-fused-large.json','bridge-fused-selected-large.json','bridge-fused-adoption.json']:assert load(name)['passed'],name
assert load('bridge-fused-selected-js-check.json')['bridge_sha256']==build['binary_sha256']
assert load('bridge-fused-selected-nagle.json')['bridge_sha256']==build['binary_sha256']
assert load('bridge-fused-selected-large.json')['sha256']==build['binary_sha256']
adopt=load('bridge-fused-adoption.json');assert adopt['bridge_sha256']==build['binary_sha256'] and adopt['source_revision']==build['source_revision']
assert all(v==0 for v in adopt['final_resource_gauges'].values())
assert '79 passed; 0 failed' in (root/'raw/bridge-fused-tests.log').read_text()
assert '82 passed; 0 failed' in (root/'raw/bridge-fused-workspace-tests.log').read_text()
print('Verified matched runtime source, four conversion cohorts, exact allocation/byte reductions, wire/counter/resource gates, clean/workspace tests, selected compatibility and local adoption receipts.')
