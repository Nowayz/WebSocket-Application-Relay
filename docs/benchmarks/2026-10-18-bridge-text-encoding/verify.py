"""Offline audit of direct UTF-8 bridge encoding and selected live acceptance."""
import hashlib,json,re,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda b:hashlib.sha256(b).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
def load(name):return json.loads((root/'raw'/name).read_text())
build=json.loads((root/'build.json').read_text());source=root/'source'
selected=(source/'selected.rs').read_text();direct=(source/'measured-direct.rs').read_text();control=(source/'control.rs').read_text()
assert control.encode()==subprocess.check_output(['git','show','bc58d9c:src/legacy_bridge.rs'])
assert selected.encode()==subprocess.check_output(['git','show',build['source_revision']+':src/legacy_bridge.rs'])
assert sha(selected.encode())==build['source_sha256']
def runtime(s):return s.split('\n#[cfg(test)]')[0].rstrip()
assert runtime(selected).replace('                // CBOR lengths count UTF-8 bytes; encode directly into final storage.\n','')==runtime(direct)
a=runtime(control);b=runtime(direct)
x=a.index('        Message::Text(text) => {');y=a.index('\n        _ => {',x)
u=b.index('        Message::Text(text) => {');v=b.index('\n        _ => {',u)
assert a[:x]==b[:u] and a[y:]==b[v:]
assert 'name = "bytes"\nversion = "1.12.1"' in subprocess.check_output(['git','show',build['source_revision']+':Cargo.lock'],text=True)
variants=['control','direct','direct','control'];cohorts=[]
for i,label in enumerate(variants):
 rows=[json.loads(line) for line in (root/'raw'/f'bridge-text-screen-{i}-{label}.jsonl').read_text().splitlines()]
 assert [r['payload_bytes'] for r in rows]==[1,16,1024,65536]
 assert all(len(r['ns_per_operation'])==9 and min(r['ns_per_operation'])>0 for r in rows);cohorts.append(rows)
rows=cohorts[0];calls=sum(1+1000+9*r['operations_per_batch'] for r in rows);assert calls==2722436
counts=[]
for label in ['control','direct']:
 header=(root/'raw'/f'heap-{label}.heap').read_text().splitlines()[0];m=re.search(r'\[(\d+):\s*(\d+)\]',header);assert m;counts.append(tuple(map(int,m.groups())))
assert tuple(a-b for a,b in zip(*counts))==(2*calls,2301343517)
assert 2301343517==sum((r['payload_bytes']+9+24)*(1+1000+9*r['operations_per_batch']) for r in rows)
def width(n):return 1 if n<24 else 2 if n<256 else 3 if n<65536 else 5 if n<2**32 else 9
for name in [f'bridge-text-wire-{i}-{l}.json' for i,l in enumerate(variants)]+['bridge-text-wire-64k.json','bridge-text-wire-1m.json','bridge-text-selected-wire-1m.json']:
 d=load(name);assert d['passed'] and d['exit_codes']==[0,0];n=d['clients'];rounds=d['rounds_per_client'];m=d['retired']['metrics']
 assert d['verified_probes']==n*rounds and m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==n
 assert m['war_incoming_messages_total']==n*rounds and m['war_deliveries_total']==n*(rounds+1)
 assert m['war_routing_reused_bytes_total']==n*rounds*(d['payload_bytes']+width(d['payload_bytes']))
 assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']) and m['war_channels']==1
 if name.startswith('bridge-text-selected'):assert d['bridge_sha256']==build['binary_sha256']
for name in ['bridge-text-js-check.json','bridge-text-selected-js-check.json','bridge-text-selected-nagle.json','bridge-text-adoption.json']:assert load(name)['passed'],name
assert load('bridge-text-selected-js-check.json')['bridge_sha256']==build['binary_sha256']
assert load('bridge-text-selected-nagle.json')['bridge_sha256']==build['binary_sha256']
adopt=load('bridge-text-adoption.json');assert adopt['bridge_sha256']==build['binary_sha256'] and adopt['source_revision']==build['source_revision']
assert all(v==0 for v in adopt['final_resource_gauges'].values())
assert 'private 1KiB UTF-8 text' in adopt['verified'] and 'room broadcast 1KiB UTF-8 text' in adopt['verified']
assert '80 passed; 0 failed' in (root/'raw/bridge-text-tests.log').read_text()
assert '83 passed; 0 failed' in (root/'raw/bridge-text-workspace-tests.log').read_text()
print('Verified matched text runtime, four conversion cohorts, exact two-allocation/byte reductions, UTF-8 wire/counter/resource gates, tests, selected interoperability and live binary/text adoption receipts.')
