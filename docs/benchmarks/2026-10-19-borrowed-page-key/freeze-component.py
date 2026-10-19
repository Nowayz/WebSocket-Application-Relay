import json,sys,hashlib,shutil
from pathlib import Path
kind=sys.argv[1];root=Path(f'artifacts/page-borrowed-key-20261019-{kind}-source').resolve();output=Path(f'artifacts/candidates/page-borrowed-key-20261019-{kind}');output.mkdir()
log=Path(f'artifacts/page-borrowed-key-20261019-{kind}-build.log');entries=[]
for line in log.read_text().splitlines():
 try:x=json.loads(line)
 except json.JSONDecodeError:continue
 if x.get('reason')=='compiler-artifact' and x.get('manifest_path')==str(root/'Cargo.toml') and x['profile']['test'] and x.get('executable') and 'lib' in x['target']['kind']:entries.append(x)
assert len(entries)==1 and entries[0]['fresh'] is False
binary=output/'page_screen';shutil.copy2(entries[0]['executable'],binary)
meta={'passed':True,'artifact':entries[0],'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'source_sha256':{str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(root.rglob('*')) if p.is_file()},'build_log_sha256':hashlib.sha256(log.read_bytes()).hexdigest()}
(output/'build.json').write_text(json.dumps(meta,indent=2)+'\n')
print(json.dumps({'passed':True,'kind':kind,'binary':str(binary)}))
