import hashlib,json,re,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
versions={'baseline':'observer-quantum','inlined':'rust-owner-fanout-all','outlined':'rust-owner-fanout-isolated','explicit':'rust-owner-fanout-explicit'}
result={}
for label,name in versions.items():
 p=ROOT/'artifacts/candidates'/name/'war-relay';text=subprocess.check_output(['nm','-S','-C',str(p)],text=True);symbols={}
 for row in text.splitlines():
  if not any(s in row for s in ['socket::Socket::flush','PeerOutput>::send','relay::Peer::enqueue_frame']):continue
  fields=row.split(None,3)
  if len(fields)!=4:continue
  address,size,kind,symbol=fields;start=int(address,16)
  asm=subprocess.check_output(['objdump','-Cd',str(p),f'--start-address={start}',f'--stop-address={start+128}'],text=True)
  match=re.search(r'sub\s+\$0x([0-9a-f]+),%rsp',asm)
  symbols[symbol]={'code_bytes':int(size,16),'stack_reservation_bytes_excluding_saved_registers':int(match.group(1),16) if match else 0,'prologue':asm}
 result[label]={'executable_sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'symbols':symbols}
(ROOT/'artifacts/rust-owner-fanout-code-layout.json').write_text(json.dumps(result,indent=2)+'\n')
for k,v in result.items():
 for name,r in v['symbols'].items():print(k,name,r['code_bytes'],r['stack_reservation_bytes_excluding_saved_registers'])
