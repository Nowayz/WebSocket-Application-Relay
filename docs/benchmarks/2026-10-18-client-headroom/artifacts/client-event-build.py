from pathlib import Path
import subprocess,json,hashlib
project=Path('artifacts/client-event-work');source=project/'examples/capacity.rs';original=source.read_text()
records=[]
for label,code in [('control',original),('8',original.replace('.worker_threads(args.client_workers)','.worker_threads(args.client_workers)\n        .event_interval(8)',1))]:
 source.write_text(code)
 command=['cargo','build','--manifest-path',str(project/'Cargo.toml'),'--release','--example','capacity']
 import os
 env=os.environ.copy();env['CARGO_TARGET_DIR']='artifacts/capacity-client-build'
 with open(f'artifacts/client-event-{label}-build.log','w') as log:subprocess.run(command,env=env,stdout=log,stderr=log,check=True)
 dest=Path('artifacts/candidates/client-event-'+label)
 for src,name in [(Path('artifacts/capacity-client-build/release/examples/capacity'),'capacity'),(source,'capacity.rs')]:
  (dest/name).write_bytes(src.read_bytes())
 records.append({'label':label,'command':command,'target':env['CARGO_TARGET_DIR'],'binary_sha256':hashlib.sha256((dest/'capacity').read_bytes()).hexdigest(),'source_sha256':hashlib.sha256((dest/'capacity.rs').read_bytes()).hexdigest()})
 Path('artifacts/client-event-build.json').write_text(json.dumps(records,indent=2)+'\n')
 print('built',label,flush=True)
source.write_text(original)
