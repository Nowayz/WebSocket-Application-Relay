import gzip,hashlib,json,shutil,subprocess,tarfile,tempfile,io
from pathlib import Path
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-ready-slot';out.mkdir(exist_ok=True)
record=json.load(open('artifacts/candidates/ready-slot/build.json'))
with tempfile.TemporaryDirectory(prefix='war-ready-slot-replay-') as temp:
    tree=Path(temp)
    archive=subprocess.check_output(['git','archive',record['base'],'--','src','tests','vendor','Cargo.toml','Cargo.lock'])
    with tarfile.open(fileobj=io.BytesIO(archive)) as contents:contents.extractall(tree,filter='data')
    subprocess.run(['git','apply',str(root/'artifacts/candidates/ready-slot/change.patch')],cwd=tree,check=True)
    for name,expected in record['sources'].items():assert hashlib.sha256((tree/name).read_bytes()).hexdigest()==expected,name
for p in (root/'artifacts').glob('ready-slot*'):
    if not p.is_file():continue
    if p.suffix=='.csv':(out/(p.name+'.gz')).write_bytes(gzip.compress(p.read_bytes(),mtime=0))
    else:shutil.copy2(p,out/p.name)
for name in ['build.json','change.patch']:shutil.copy2(root/'artifacts/candidates/ready-slot'/name,out/name)
for name in ['scripts/inspection.py','scripts/capacity.py','scripts/benchmark.py','scripts/capacity_pressure.py']:
    p=root/name;dest=out/'scripts'/p.name;dest.parent.mkdir(exist_ok=True);shutil.copy2(p,dest)
cohorts={}
for name in ['private','capacity']:
    p=json.load(open(f'artifacts/ready-slot-{name}.json'));assert p['passed'] and len(p['runs'])==6
    cohorts[name]=p['summary']
for name in ['control','slot','slot-reverse','control-reverse','no-heartbeat']:
    assert json.load(open(f'artifacts/ready-slot-fairness-{name}.json'))['passed']
assert json.load(open('artifacts/ready-slot-mixed.json'))['passed']
manifest={'scope':'Deferred ready-slot cache. 2.9 percent lower median CPU at 100000-user offered overload, but higher-rate per-user tail latency changes rank across two orders. Not selected; original runtime source and exact 35d46da1 executable restored. One heartbeat-disabled control is diagnostic, not a recommended operating policy or causal proof. No local manual service change.', 'base_revision':record['base'],'sources_replayed_and_verified':len(record['sources']),'cohorts':cohorts,'fairness':json.load(open('artifacts/ready-slot-fairness-summary.json')),'files':{str(p.relative_to(out)):hashlib.sha256(p.read_bytes()).hexdigest() for p in out.rglob('*') if p.is_file()}}
out.with_suffix('.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Verified source replay and archived',len(manifest['files']),'files')
