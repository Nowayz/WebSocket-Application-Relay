import gzip,hashlib,json,shutil,subprocess,tempfile,tarfile,io
from pathlib import Path
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-latency-timeline';out.mkdir(exist_ok=True)
record=json.load(open('artifacts/candidates/latency-timeline-client/build.json'))
assert hashlib.sha256(Path('examples/capacity.rs').read_bytes()).hexdigest()==record['source_sha256']
with tempfile.TemporaryDirectory(prefix='war-latency-timeline-replay-') as temp:
    tree=Path(temp)
    archive=subprocess.check_output(['git','archive',record['base_revision'],'--','examples/capacity.rs'])
    with tarfile.open(fileobj=io.BytesIO(archive)) as contents:contents.extractall(tree,filter='data')
    subprocess.run(['git','apply',str(root/'artifacts/candidates/latency-timeline-client/change.patch')],cwd=tree,check=True)
    assert hashlib.sha256((tree/'examples/capacity.rs').read_bytes()).hexdigest()==record['source_sha256']
cohorts={}
for label in ['on-1','off-1','off-2','on-2']:
    data=json.load(open(f'artifacts/latency-timeline-{label}.json'))
    summary=json.load(open(f'artifacts/latency-timeline-{label}-summary.json'))
    assert data['passed'] and data['client_sha256']==record['binary_sha256']
    assert data['server_sha256']=='35d46da18a5eb4e945d5eacb7962581e520b7e613e9006d89db559264e63db6e'
    assert data['server_exit_code']==data['client_exit_code']==0
    cohorts[label]={k:v for k,v in summary.items() if k!='timeline'}
for p in (root/'artifacts').glob('latency-timeline*'):
    if not p.is_file():continue
    if p.suffix=='.csv':(out/(p.name+'.gz')).write_bytes(gzip.compress(p.read_bytes(),mtime=0))
    else:shutil.copy2(p,out/p.name)
for name in ['build.json','change.patch','capacity.rs']:
    dest=out/'sources'/name;dest.parent.mkdir(exist_ok=True);shutil.copy2(root/'artifacts/candidates/latency-timeline-client'/name,dest)
for name in ['scripts/capacity.py','scripts/benchmark.py','scripts/capacity_pressure.py','scripts/plot-latency-timeline.py','Cargo.lock']:
    dest=out/'sources'/Path(name).name;shutil.copy2(root/name,dest)
manifest={'scope':'Diagnostic timeline and observed Ping-cohort comparison on unchanged selected Mio server. Four sequential ABBA heartbeat on/off cases, 100000 users, 400000 offered 1KiB private packets/s, 30 seconds/120 samples per user, four server/eight client cores. Exact one-second intended-send cohorts, after-timing aggregation. No runtime optimization or heartbeat policy selected. Client scheduling and room batching included. Ping counts are probe-encounter counts, not kernel timestamps. Not causal or production SLO proof.', 'base_revision':record['base_revision'],'source_replay_verified':True,'cohorts':cohorts,'files':{str(p.relative_to(out)):hashlib.sha256(p.read_bytes()).hexdigest() for p in out.rglob('*') if p.is_file()}}
out.with_suffix('.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Archived',len(manifest['files']),'verified source and evidence files')
