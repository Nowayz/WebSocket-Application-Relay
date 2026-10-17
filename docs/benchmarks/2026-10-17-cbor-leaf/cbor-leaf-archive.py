import hashlib,json,shutil,subprocess,tarfile,tempfile,io
from pathlib import Path
root=Path.cwd(); out=root/'docs/benchmarks/2026-10-17-cbor-leaf';out.mkdir(exist_ok=True)
record=json.load(open('artifacts/candidates/cbor-leaf/build.json'))
with tempfile.TemporaryDirectory(prefix='war-cbor-leaf-replay-') as temp:
    tree=Path(temp)
    archive=subprocess.check_output(['git','archive',record['base'],'--','src','tests','vendor','Cargo.toml','Cargo.lock'])
    with tarfile.open(fileobj=io.BytesIO(archive)) as contents:contents.extractall(tree,filter='data')
    subprocess.run(['git','apply',str(root/'artifacts/candidates/cbor-leaf/change.patch')],cwd=tree,check=True)
    for name,expected in record['sources'].items():assert hashlib.sha256((tree/name).read_bytes()).hexdigest()==expected,name
for p in (root/'artifacts').glob('cbor-leaf*'):
    if p.is_file():shutil.copy2(p,out/p.name)
for name in ['build.json','change.patch','protocol.rs']:shutil.copy2(root/'artifacts/candidates/cbor-leaf'/name,out/name)
for name in ['scripts/inspection.py','scripts/capacity.py','scripts/benchmark.py','scripts/capacity_pressure.py']:
    p=root/name;dest=out/'scripts'/p.name;dest.parent.mkdir(exist_ok=True);shutil.copy2(p,dest)
cohorts={}
for name in ['private','capacity']:
    p=json.load(open(f'artifacts/cbor-leaf-{name}.json'));assert p['passed'] and len(p['runs'])==6
    cohorts[name]=p['summary']
manifest={'scope':'Rejected CBOR leaf-validation ablation; source restored. Both candidates use the native Mio runtime and worker receive storage; inherited generic completion wording in raw runner scope is not a description of either backend here. Only source change is archived parser patch. No per-user candidate fairness run: candidate not selected.', 'base_revision':record['base'],'sources_replayed_and_verified':len(record['sources']),'cohorts':cohorts,'files':{str(p.relative_to(out)):hashlib.sha256(p.read_bytes()).hexdigest() for p in out.rglob('*') if p.is_file()}}
out.with_suffix('.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Verified source replay and archived',len(manifest['files']),'files')
