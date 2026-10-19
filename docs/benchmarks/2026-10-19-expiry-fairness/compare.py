import subprocess,json,hashlib
from pathlib import Path
output=Path('artifacts/expiry-burst-20261019-comparison-v4.json');assert not output.exists()
record={'passed':False,'runs':[]}
for index,(label,kind) in enumerate([('control-1','control'),('selected-1','selected'),('selected-2','selected'),('control-2','control')]):
 binary=Path(f'artifacts/candidates/expiry-burst-20261019-{kind}/war-relay')
 receipt=Path(f'artifacts/expiry-burst-20261019-v4-{label}.json')
 command=['python','artifacts/expiry-burst-20261019-ordinary-v4.py','--server',str(binary),'--users','100000','--source-prefix',f'127.{224+index}.0','--output',str(receipt)]
 with open(receipt.with_suffix('.controller.log'),'x') as log: done=subprocess.run(command,stdout=log,stderr=log)
 x=json.loads(receipt.read_text());assert done.returncode==0 and x['passed'],(label,x.get('error'))
 assert x['timer_bursts']==[] and not x['diagnostic_only']
 row={'label':label,'kind':kind,'command':command,'receipt':str(receipt),'receipt_sha256':hashlib.sha256(receipt.read_bytes()).hexdigest(),'first_post_resume_probe_ms':x['first_post_resume_probe_ms'],'ramp_seconds':x['ramp_seconds']}
 record['runs'].append(row);output.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True;output.write_text(json.dumps(record,indent=2)+'\n')
