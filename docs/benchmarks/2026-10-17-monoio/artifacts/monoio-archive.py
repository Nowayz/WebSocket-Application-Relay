"""Preserve complete Monoio trial evidence and replayable source patches."""
import difflib,hashlib,io,json,shutil,subprocess,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'docs/benchmarks/2026-10-17-monoio'
OUT.mkdir(parents=True,exist_ok=True)
BASE='67e460905b0a55263d89a39657fff471e61cd40e'
sha=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
archive=subprocess.check_output(['git','archive','--format=tar',BASE,'--','Cargo.toml','Cargo.lock','src','vendor','tests','examples'],cwd=ROOT)
index={'base_revision':BASE,'variants':{}}
with tempfile.TemporaryDirectory(prefix='war-monoio-base-',dir='/tmp') as tmp:
    original=Path(tmp)/'original';original.mkdir()
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:tar.extractall(original,filter='data')
    for label,candidate in [('readiness','monoio-readiness'),('persistent-unfixed','monoio-persistent'),('persistent','monoio-final')]:
        frozen=ROOT/'artifacts/candidates'/candidate
        build=json.loads((frozen/'build.json').read_text())
        source=ROOT/build['source_root']
        for name,expected in build['sources'].items():assert sha(source/name)==expected,(label,name)
        assert sha(frozen/'war-relay')==build['binaries']['war-relay']
        patch=[]
        for name in sorted(build['sources']):
            before=original/name;after=source/name
            old=before.read_text() if before.exists() else ''
            new=after.read_text()
            if old!=new:
                patch.extend(difflib.unified_diff(old.splitlines(keepends=True),new.splitlines(keepends=True),fromfile='a/'+name if before.exists() else '/dev/null',tofile='b/'+name))
        path=OUT/'sources'/f'{label}.patch';path.parent.mkdir(exist_ok=True)
        path.write_text(''.join(patch))
        replay=Path(tmp)/label;shutil.copytree(original,replay)
        subprocess.run(['git','apply',str(path)],cwd=replay,check=True)
        for name,expected in build['sources'].items():assert sha(replay/name)==expected,(label,name,'replay')
        metadata=OUT/'builds'/f'{label}.json';metadata.parent.mkdir(exist_ok=True);shutil.copyfile(frozen/'build.json',metadata)
        index['variants'][label]={'patch':{'path':str(path.relative_to(ROOT/'docs')),'sha256':sha(path)},'sources':build['sources'],'observed_binaries':build['binaries'],'source_replay_verified':True,'description':build['variant']}
(OUT/'sources.json').write_text(json.dumps(index,indent=2)+'\n')
files=[]
for path in sorted((ROOT/'artifacts').glob('monoio-*')):
    if path.name.endswith('.perf.data'):continue
    files.extend([path] if path.is_file() else [p for p in path.rglob('*') if p.is_file()])
for rel in ['scripts/try-monoio.py','scripts/inspection.py','scripts/benchmark.py','scripts/capacity.py','scripts/capacity_pressure.py','scripts/compare.py','scripts/stability.py','scripts/input-corpus.py','scripts/fragment-waves.py','scripts/reference.py']:
    files.append(ROOT/rel)
for path in files:
    target=OUT/path.relative_to(ROOT);target.parent.mkdir(parents=True,exist_ok=True)
    shutil.copyfile(path,target);assert sha(target)==sha(path)
for name,count in [('monoio-readiness-private',6),('monoio-persistent-private',15),('monoio-capacity',9),('monoio-final-private',6),('monoio-final-capacity',6),('monoio-final-mixed',2)]:
    d=json.loads((ROOT/f'artifacts/{name}.json').read_text());assert d['passed'] and len(d['runs'])==count,name
    for row in d['runs']:
        if 'trace' in row:assert sha(ROOT/row['trace'])==row['trace_sha256']
fanout=json.loads((ROOT/'artifacts/monoio-final-fanout.json').read_text())
assert len(fanout['runs'])==6 and all(r['passed'] for r in fanout['runs'])
for variant in ['readiness','persistent','final']:
    for check in ['corpus','fragments']:
        assert json.loads((ROOT/f'artifacts/monoio-{variant}-{check}.json').read_text())['case']['passed']
assert json.loads((ROOT/'artifacts/monoio-driver-proof.json').read_text())['passed']
print(json.dumps({'source_replays':len(index['variants']),'archived_files':len(files),'source_index':str((OUT/'sources.json').relative_to(ROOT))}))
