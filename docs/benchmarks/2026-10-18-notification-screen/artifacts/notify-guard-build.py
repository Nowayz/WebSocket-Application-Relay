from pathlib import Path
import subprocess,os,json,hashlib
project=Path('artifacts/notify-guard-work');source=project/'src/server/socket.rs';original=source.read_text()
candidate=original.replace('use parking_lot::Mutex;','use parking_lot::{Mutex, MutexGuard};',1)
candidate=candidate.replace('''    fn notify(&self) {
        if !self.notified.swap(true, Relaxed) {''','''    // Producers hold this socket's State lock. The owner clears the flag
    // before driving under that same lock: it either consumes this submission
    // or leaves a queued wake. Producer serialization needs no atomic RMW.
    fn notify(&self, _state: &MutexGuard<'_, State>) {
        if !self.notified.load(Relaxed) {
            self.notified.store(true, Relaxed);''',1)
candidate=candidate.replace('self.notify();','self.notify(&state);')
assert candidate.count('self.notify(&state);')==3
records=[]
for label,text in [('control',original),('candidate',candidate)]:
 source.write_text(text)
 env=os.environ.copy();env['CARGO_TARGET_DIR']='artifacts/capacity-client-build'
 command=['cargo','build','--manifest-path',str(project/'Cargo.toml'),'--release','--bin','war-relay']
 with open(f'artifacts/notify-guard-{label}-build.log','w') as log:subprocess.run(command,env=env,stdout=log,stderr=log,check=True)
 dest=Path('artifacts/candidates/notify-guard-'+label)
 for src,name in [(Path('artifacts/capacity-client-build/release/war-relay'),'war-relay'),(source,'socket.rs')]:
  (dest/name).write_bytes(src.read_bytes())
 (dest/'war-relay').chmod(0o755)
 records.append({'label':label,'command':command,'target':env['CARGO_TARGET_DIR'],'binary_sha256':hashlib.sha256((dest/'war-relay').read_bytes()).hexdigest(),'socket_source_sha256':hashlib.sha256((dest/'socket.rs').read_bytes()).hexdigest()})
 Path('artifacts/notify-guard-build.json').write_text(json.dumps(records,indent=2)+'\n')
 print('built',label,flush=True)
