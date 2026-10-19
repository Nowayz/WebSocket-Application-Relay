"""Exact requested bridge heap accounting with executable stack attribution."""
import hashlib,json,re,subprocess
from pathlib import Path
ROOT=Path.cwd();result={'scope':'Requested live/cumulative bridge heap under replacement allocator; not RSS or ordinary timing. Every raw row sums to its header. Generic/unresolved frames are preserved without inventing callers.','cases':{}}
pattern=re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@\s*(.*)')
keys=['live_objects','live_bytes','allocated_objects','allocated_bytes']
for variant in ['control','candidate']:
 receipt=json.load(open(f'artifacts/bridge-join-owner-{variant}-heap.json'));binary=Path(receipt['commands'][1][0]);addresses=set();captures=[]
 assert hashlib.sha256(binary.read_bytes()).hexdigest()==receipt['bridge_sha256']
 for captured in receipt['heap_profiler']['captures']:
  data=Path(captured['path']).read_bytes();assert hashlib.sha256(data).hexdigest()==captured['sha256']
  lines=data.decode().splitlines();maps=[s.split() for s in lines if str(binary) in s]
  base=int(next(m[0].split('-')[0] for m in maps if int(m[2],16)==0),16)
  segments=[tuple(int(a,16) for a in m[0].split('-')) for m in maps];rows=[]
  for line in lines[1:]:
   m=pattern.fullmatch(line)
   if not m:continue
   offsets=[int(a,16)-base-1 for a in m[5].split() if any(lo<=int(a,16)<hi for lo,hi in segments)];addresses.update(offsets)
   rows.append({**dict(zip(keys,[int(m[i]) for i in range(1,5)])),'offsets':[hex(a) for a in offsets]})
  header=pattern.fullmatch(lines[0].removeprefix('heap profile:'));assert header
  totals=dict(zip(keys,[int(header[i]) for i in range(1,5)]));assert totals=={k:sum(row[k] for row in rows) for k in keys}
  captures.append({'phase':captured['phase'],'heap_sha256':captured['sha256'],'base':hex(base),'totals':totals,'rows':rows})
 output=subprocess.run(['addr2line','-a','-f','-C','-i','-e',str(binary)],input=''.join(hex(a)+'\n' for a in sorted(addresses)),capture_output=True,text=True,check=True)
 symbols={};current=None
 for line in output.stdout.splitlines():
  if re.fullmatch(r'0x[0-9a-fA-F]+',line):current=hex(int(line,16));symbols[current]=[]
  elif current is not None:symbols[current].append(line)
 for captured in captures:
  groups={}
  for row in captured['rows']:
   names=[s for a in row['offsets'] for s in symbols.get(a,[])]
   name=next((s for s in names if s!='??' and ':?' not in s and ':0' not in s),'outside executable / unresolved')
   group=groups.setdefault(name,dict.fromkeys(keys,0))
   for k in keys:group[k]+=row[k]
  captured['groups']=dict(sorted(groups.items(),key=lambda x:x[1]['live_bytes'],reverse=True))
 result['cases'][variant]={'binary':str(binary),'binary_sha256':receipt['bridge_sha256'],'captures':captures,'symbols':symbols}
Path('artifacts/bridge-join-owner-analysis.json').write_text(json.dumps(result,indent=2)+'\n')
for v,c in result['cases'].items():
 warmed=next(x for x in c['captures'] if x['phase']=='warmed');print(v,warmed['totals'])
 print({name:counts for name,counts in warmed['groups'].items() if 'reserve_inner' in name or 'shallow_clone' in name})
