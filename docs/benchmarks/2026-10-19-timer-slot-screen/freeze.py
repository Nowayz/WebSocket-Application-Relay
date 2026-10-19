import hashlib,json,shutil,sys
from pathlib import Path
kind=sys.argv[1];root=Path(f'artifacts/timer-slot-20261019-{kind}-source').resolve()
messages=[]
for line in Path(f'artifacts/timer-slot-20261019-{kind}-build.log').read_text().splitlines():
 try:x=json.loads(line)
 except json.JSONDecodeError:continue
 if x.get('reason')=='compiler-artifact' and x.get('manifest_path')==str(root/'Cargo.toml') and x.get('executable'):
  messages.append(x)
assert len(messages)==1 and messages[0]['fresh'] is False,messages
p=Path(f'artifacts/candidates/timer-slot-20261019-{kind}');p.mkdir()
shutil.copy2(messages[0]['executable'],p/'measurement')
receipt={'base_commit':'3f65efd','kind':kind,'cargo_artifact':messages[0], 'binary_sha256':hashlib.sha256((p/'measurement').read_bytes()).hexdigest(),'sources':{str(f.relative_to(root)):hashlib.sha256(f.read_bytes()).hexdigest() for f in root.rglob('*') if f.is_file()}}
(p/'build.json').write_text(json.dumps(receipt,indent=2)+'\n')
