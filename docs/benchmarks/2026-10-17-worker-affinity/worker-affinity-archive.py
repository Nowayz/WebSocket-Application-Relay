from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();dst=root/'docs/benchmarks/2026-10-17-worker-affinity';dst.mkdir(exist_ok=True)
def copy(p,name=None):
    target=dst/(name or p.name)
    if p.suffix=='.csv': target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
    else:shutil.copyfile(p,target)
for p in sorted((root/'artifacts').glob('worker-affinity-*')):
    if p.is_file() and p.suffix in ['.json','.log','.csv','.py','.png','.pdf'] and p.name!='worker-affinity-document.py':copy(p)
for p in (root/'artifacts/candidates/worker-affinity').iterdir():
    if p.suffix in ['.json','.patch']:copy(p,'candidate-'+p.name)
for name in ['scripts/capacity.py','scripts/analyze-scheduling.py','scripts/test-scheduling-analysis.py','scripts/reference.py','scripts/cbor-corpus.py']:
    p=root/name
    if p.exists():copy(p,'source-'+p.name)
for name in ['affinity-latency-summary.json','affinity-latency-capacity-harness.py','affinity-latency-run.py','scheduling-100k-on-1.json','scheduling-100k-on-1-analysis.json','scheduling-100k-on-1-analysis.log','scheduling-100k-on-1-report.log','scheduling-analysis-tests.log']:
    p=root/'artifacts'/name
    if p.exists():copy(p)
for p in sorted((root/'artifacts').glob('affinity-latency-*-?.json')):copy(p)
for folder in ['latency-timeline-client']:
    for p in (root/'artifacts/candidates'/folder).iterdir():
        if p.suffix in ['.json','.rs','.patch']:copy(p,folder+'-'+p.name)
build=json.loads((dst/'candidate-build.json').read_text())
for name,digest in build['sources'].items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==digest,name
for p in (root/'artifacts').glob('worker-affinity-latency-*-?.json'):
    x=json.loads(p.read_text());assert x['runner_sha256']==hashlib.sha256((root/'scripts/capacity.py').read_bytes()).hexdigest()
manifest={'scope':'Native startup CPU affinity, same binary off/on/on/off at 100000 users; exact sources and raw traces. Initial harness intervention and scoped scheduling trace are diagnostic context. Raw large perf/events files remain local and are not in Git. No production or C++ comparison implied.','candidate_binary_sha256':build['binary_sha256'],'base_revision':build['base_revision'],'files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(dst.iterdir()) if p.name!='manifest.json'}}
(dst/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'archived_files':len(manifest['files']),'bytes':sum(p.stat().st_size for p in dst.iterdir())}))
