from pathlib import Path
import hashlib,json,subprocess,tempfile,tarfile,io,shutil
p=Path('docs/benchmarks/2026-10-17-observer-quantum');m=json.loads((p/'manifest.json').read_text())
for name,want in m['files'].items():assert hashlib.sha256((p/name).read_bytes()).hexdigest()==want,name
verified=[]
for prefix,patch,checks_key in [('server','runtime.patch','sources'),('observer-client','change.patch','sources'),('baseline','runtime.patch','sources'),('capacity-client','change.patch',None),('mixed-clients','runtime.patch','files')]:
 b=json.loads((p/(prefix+'-build.json')).read_text())
 names=set(subprocess.check_output(['git','ls-tree','--name-only',b['base_revision']],text=True).splitlines());paths=[n for n in ['src','tests','vendor','examples','scripts','deploy','Cargo.toml','Cargo.lock'] if n in names]
 with tempfile.TemporaryDirectory(prefix='war-observer-'+prefix+'-replay-') as t:
  data=subprocess.check_output(['git','archive',b['base_revision'],*paths])
  with tarfile.open(fileobj=io.BytesIO(data)) as f:f.extractall(t,filter='data')
  subprocess.run(['git','apply',str((p/(prefix+'-'+patch)).resolve())],cwd=t,check=True)
  if prefix=='mixed-clients':
   for original,archived in m['mixed_build_overlays'].items():
    dest=Path(t)/original;assert not dest.exists();dest.parent.mkdir(exist_ok=True,parents=True);shutil.copyfile(p/archived,dest)
  checks=b[checks_key] if checks_key else {'examples/capacity.rs':b['source_sha256'],'Cargo.lock':b['cargo_lock_sha256']}
  for name,want in checks.items():
   if checks_key=='files' and not name.endswith(('.rs','.py','.toml','.lock')):continue
   assert hashlib.sha256((Path(t)/name).read_bytes()).hexdigest()==want,(prefix,name)
 verified.append(prefix)
result={'passed':True,'build_source_replays':verified,'mixed_overlay_scope':'The old mixed-client build included three then-untracked source files absent from its base+diff. These are preserved originals with matching build-record hashes,not reconstructed substitutes.','archive_hashes_verified':len(m['files'])}
Path('artifacts/observer-quantum-source-replay.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
