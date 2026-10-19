"""Audit static request bytes, release identities, timings and raw allocations."""
import gzip,hashlib,json,re,statistics
from pathlib import Path
p=Path(__file__).resolve().parent
read=lambda name:json.loads((p/name).read_text())
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in read('sha256.json').items():assert sha((p/name).read_bytes())==digest,name
base=gzip.decompress((p/'base-legacy-bridge.rs.gz').read_bytes())
component=gzip.decompress((p/'component-candidate-legacy-bridge.rs.gz').read_bytes())
selected=gzip.decompress((p/'selected-legacy-bridge.rs.gz').read_bytes())
assert base.replace(b'legacy::Command::Channels => protocol::request(2, &[0xf6]),',b'// The complete request is always [0, [2, null]].\n                legacy::Command::Channels => Bytes::from_static(b"\\x82\\x00\\x82\\x02\\xf6"),')==component
assert component.split(b'#[cfg(test)]',1)[0]==selected.split(b'#[cfg(test)]',1)[0]
for kind,source in [('control',base),('candidate',component),('selected',selected)]:
 build=read(kind+'-build.json');assert build['passed'] and build['cargo_artifact']['fresh'] is False
 assert build['source_sha256']['src/legacy_bridge.rs']==sha(source)
 if kind!='selected':assert build['source_sha256']['examples/list_request_conversion.rs']==sha((p/'conversion-probe.rs').read_bytes())
timing=read('timing.json');assert timing['passed'] and len(timing['runs'])==20
for op in [2,1,0,5,4]:
 rows=[r for r in timing['runs'] if r['result']['op']==op]
 assert [r['label'] for r in rows]==['control-1','candidate-1','candidate-2','control-2']
 for r in rows:
  result=r['result'];assert len(result['samples_ns'])==7 and statistics.median(result['samples_ns'])==result['ns_per_conversion']
  assert result['iterations_per_batch']==(10000 if op==4 else 100000)
allocation=read('allocation.json');assert not any(allocation['caller_profiler_environment_present'].values())
for kind in ['control','candidate']:
 receipt=read(kind+'-heap.json');assert receipt['passed'] and receipt['exit_code']==0
 assert receipt['server_sha256']==read(kind+'-build.json')['binary_sha256']
 headers=[]
 for phase in ['before','after']:
  body=gzip.decompress((p/(kind+'-'+phase+'.heap.gz')).read_bytes())
  assert sha(body)==receipt['probes'][0][phase]['sha256']
  lines=body.decode().splitlines()
  header=list(map(int,re.search(r'heap profile:\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]',lines[0]).groups()))
  pattern=re.compile(r'^\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@')
  rows=[tuple(map(int,m.groups())) for line in lines[1:] if (m:=pattern.match(line))]
  assert [sum(r[i] for r in rows) for i in range(4)]==header
  headers.append(header)
 delta=[b-a for a,b in zip(*headers)]
 assert delta==([0,0,1000,5000] if kind=='control' else [0,0,0,0])
 assert delta==allocation['variants'][kind]['delta_live_objects_live_bytes_allocations_allocated_bytes']
assert '139 passed; 0 failed' in (p/'tests.log').read_text()
assert 'Finished' in (p/'clippy.log').read_text()
js=(p/'js.log').read_text();assert 'pass 65' in js and 'fail 0' in js and 'all six checked gauges are zero' in js
print('PASS: constant packet, release source identities, 20 timing processes, raw 1000-to-zero allocation evidence, 139 library/65 JS checks')
