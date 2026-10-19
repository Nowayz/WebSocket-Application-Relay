#!/usr/bin/env python3
"""Audit ownership-only scope, exact heap attribution and ordinary memory repeats."""
import gzip,hashlib,json,re
from pathlib import Path
ROOT=Path(__file__).resolve().parent
for line in (ROOT/'SHA256SUMS').read_text().splitlines():
 expected,name=line.split('  ',1)
 assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest()==expected,name

def read(name):return gzip.decompress((ROOT/'raw'/(name+'.gz')).read_bytes())
original=gzip.decompress((ROOT/'original-legacy_bridge.rs.gz').read_bytes())
selected=gzip.decompress((ROOT/'selected-legacy_bridge.rs.gz').read_bytes())
old=b'''    upstream
        .send(Message::Binary(protocol::byte_string(&joined)))
        .await?;'''
new=b'''    let subscription = protocol::byte_string(&joined);
    // The encoded subscription owns its bytes; release the frontend join view.
    drop(joined);
    upstream.send(Message::Binary(subscription)).await?;'''
assert selected.count(new)==1 and selected.replace(new,old)==original
builds={v:json.loads((ROOT/(v+'-build.json')).read_text()) for v in ['control','candidate']}
for v,data in [('control',original),('candidate',selected)]:
 assert builds[v]['passed'] and builds[v]['cargo_artifact']['fresh'] is False
 assert hashlib.sha256(data).hexdigest()==builds[v]['source_sha256']['src/legacy_bridge.rs']
assert {n for n in builds['control']['source_sha256'].keys()|builds['candidate']['source_sha256'].keys()
        if builds['control']['source_sha256'].get(n)!=builds['candidate']['source_sha256'].get(n)}=={'src/legacy_bridge.rs'}
pattern=re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@\s*(.*)')
keys=['live_objects','live_bytes','allocated_objects','allocated_bytes']
analysis=json.loads((ROOT/'bridge-join-owner-analysis.json').read_text())
profiles={};total=0

def checked(record,variant):
 global total
 assert record['passed'] and record['exit_codes']==[0,0]
 assert record['bridge_sha256']==builds[variant]['binary_sha256']
 assert record['clients']==256 and record['rounds_per_client']==8 and record['verified_probes']==2048
 m=record['retired']['metrics']
 assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==256
 assert m['war_incoming_messages_total']==2048 and m['war_deliveries_total']==2304
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_routing_copied_bytes_total','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:assert m[k]==0,k
 total+=record['verified_probes']

for v in ['control','candidate']:
 r=json.loads((ROOT/('bridge-join-owner-'+v+'-heap.json')).read_text());checked(r,v)
 assert r['profile_diagnostic'] and len(r['heap_profiler']['captures'])==4
 assert r['baseline']['bridge']['fds']==r['retired']['bridge']['fds']==10
 assert r['heap_profiler']['wrapper_sha256']==hashlib.sha256((ROOT/'bridge-join-owner-memory.py').read_bytes()).hexdigest()
 assert r['heap_profiler']['base_runner_sha256']==hashlib.sha256((ROOT/'bridge-buffer-memory-probe.py').read_bytes()).hexdigest()
 assert r['heap_profiler']['dump_helper_sha256']==hashlib.sha256((ROOT/'inspection.py').read_bytes()).hexdigest()
 case=analysis['cases'][v];assert case['binary_sha256']==r['bridge_sha256']
 phases={}
 for i,phase in enumerate(['baseline','joined','warmed','retired']):
  raw=read(v+'-'+phase+'.heap');capture=case['captures'][i]
  assert hashlib.sha256(raw).hexdigest()==capture['heap_sha256']==r['heap_profiler']['captures'][i]['sha256']
  lines=raw.decode().splitlines();h=pattern.fullmatch(lines[0].removeprefix('heap profile:'));assert h
  rows=[pattern.fullmatch(line) for line in lines[1:]];rows=[r for r in rows if r]
  totals=dict(zip(keys,[int(h[i]) for i in range(1,5)]))
  assert totals=={k:sum(int(row[i+1]) for row in rows) for i,k in enumerate(keys)}==capture['totals']
  assert totals=={k:sum(row[k] for row in capture['rows']) for k in keys}
  assert totals=={k:sum(row[k] for row in capture['groups'].values()) for k in keys}
  phases[phase]=capture
 profiles[v]=phases
for phase in ['baseline','joined','retired']:
 assert profiles['control'][phase]['totals']['live_bytes']==profiles['candidate'][phase]['totals']['live_bytes']
a,b=profiles['control']['warmed'],profiles['candidate']['warmed']
assert a['totals']['live_bytes']-b['totals']['live_bytes']==1058816
assert a['totals']['live_objects']-b['totals']['live_objects']==512
changed={n for n in a['groups'].keys()|b['groups'].keys() if a['groups'].get(n,{}).get('live_bytes',0)!=b['groups'].get(n,{}).get('live_bytes',0)}
assert changed=={'bytes::bytes_mut::BytesMut::reserve_inner','bytes::bytes_mut::BytesMut::split_to'}
assert a['groups']['bytes::bytes_mut::BytesMut::reserve_inner']['live_bytes']==1048576
for key,expected in [('allocated_bytes',1058816),('allocated_objects',512)]:
 assert (a['totals'][key]-profiles['control']['joined']['totals'][key])-(b['totals'][key]-profiles['candidate']['joined']['totals'][key])==expected
rows=json.loads((ROOT/'summary.json').read_text());assert [r['variant'] for r in rows]==['control','candidate','candidate','control']
for row in rows:
 r=json.loads(read(row['receipt']));checked(r,row['variant'])
 assert row['warmed_rss_kib']==r['warmed']['bridge']['VmRSS']
 assert row['warmed_anon_kib']==r['warmed']['bridge']['RssAnon']
assert total==12288
assert max(r['warmed_rss_kib'] for r in rows if r['variant']=='candidate')<min(r['warmed_rss_kib'] for r in rows if r['variant']=='control')
assert json.loads((ROOT/'environment.json').read_text())['ordinary_parent_LD_PRELOAD'] is None
assert '106 passed; 0 failed' in (ROOT/'bridge-join-owner-tests.log').read_text()
assert 'Finished' in (ROOT/'bridge-join-owner-clippy.log').read_text()
js=json.loads((ROOT/'bridge-join-owner-js-live-check.json').read_text())
assert js['passed'] and js['test_exit_code']==0 and js['exit_codes']==[0,0]
assert js['bridge_sha256']==builds['candidate']['binary_sha256']
assert 'pass 61' in js['test_output'] and all(v==0 for v in js['final_resource_gauges'].values())
print('PASS: ownership-only patch, eight exact heap captures, 1058816-byte/512-object saving, four ordinary repeats, 12288 probes and full conformance')
