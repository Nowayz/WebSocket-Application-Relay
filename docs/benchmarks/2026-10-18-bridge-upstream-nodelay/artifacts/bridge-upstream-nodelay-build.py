from pathlib import Path
import subprocess,os,hashlib,json
project=Path('artifacts/bridge-upstream-nodelay-work');source=project/'src/legacy_bridge.rs';control=source.read_text()
candidate=control.replace('accept_hdr_async_with_config, connect_async,','accept_hdr_async_with_config, connect_async_with_config,',1)
candidate=candidate.replace('    let (mut upstream, _) = timeout(WAIT, connect_async(upstream)).await??;','''    // Forward small realtime packets without waiting for the prior write's ACK.
    let (mut upstream, _) =
        timeout(WAIT, connect_async_with_config(upstream, None, true)).await??;''',1)
assert candidate!=control
records=[]
for label,text in [('control',control),('candidate',candidate)]:
 source.write_text(text)
 env=os.environ.copy();env['CARGO_TARGET_DIR']='artifacts/capacity-client-build'
 command=['cargo','build','--manifest-path',str(project/'Cargo.toml'),'--release','--bin','war-legacy-bridge']
 with open(f'artifacts/bridge-upstream-nodelay-{label}-build.log','w') as log:subprocess.run(command,env=env,stdout=log,stderr=log,check=True)
 dest=Path('artifacts/candidates/bridge-upstream-nodelay-'+label)
 for src,name in [(Path('artifacts/capacity-client-build/release/war-legacy-bridge'),'war-legacy-bridge'),(source,'legacy_bridge.rs')]:
  (dest/name).write_bytes(src.read_bytes())
 (dest/'war-legacy-bridge').chmod(0o755)
 records.append({'label':label,'command':command,'target':env['CARGO_TARGET_DIR'],'source_sha256':hashlib.sha256((dest/'legacy_bridge.rs').read_bytes()).hexdigest(),'binary_sha256':hashlib.sha256((dest/'war-legacy-bridge').read_bytes()).hexdigest()})
 Path('artifacts/bridge-upstream-nodelay-build.json').write_text(json.dumps(records,indent=2)+'\n')
 print('built',label,flush=True)
