#!/usr/bin/env python3
"""Freeze checked results and the exact sources/build provenance, excluding ELFs."""
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT/'docs/benchmarks/2026-10-17-threeway-current'
def sha(p): return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def read(p): return json.loads(Path(p).read_text())
def copy(p, relative):
    target=DEST/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,target)
def put(relative, content):
    target=DEST/relative;target.parent.mkdir(parents=True,exist_ok=True);target.write_text(content)

matrix=read(ROOT/'artifacts/threeway-current.json')
summary=read(ROOT/'artifacts/threeway-current-summary.json')
assert matrix.get('completed') and summary['completed'] and len(matrix['cases'])==54
assert sha(ROOT/'scripts/compare-cpp.py')==matrix['runner_sha256']
assert not DEST.exists(), 'archive already exists'
DEST.mkdir(parents=True)
for p in sorted((ROOT/'artifacts').glob('threeway-current*')):
    if not p.is_file() or p.suffix not in ('.json','.log','.py','.png','.pdf'):continue
    # Keep preliminary diagnostic failures, explicitly excluded from summary.
    copy(p,Path('raw')/p.name)
for name in ('compare-cpp.py','benchmark.py','inspection.py','reference.py'):
    copy(ROOT/'scripts'/name,Path('source/scripts')/name)
for p in (ROOT/'artifacts/baselines/active-pairs-client/examples').rglob('*.rs'):
    copy(p,Path('source/client/examples')/p.relative_to(ROOT/'artifacts/baselines/active-pairs-client/examples'))
copy(ROOT/'artifacts/reference/build-4/build.json','provenance/original-build.json')
copy(ROOT/'artifacts/reference/build-4/relay.cpp','source/original/configured-relay.cpp')
original=matrix['cpp_build']
for name, expected in original['upstream_sources'].items():
    assert sha(ROOT/name)==expected
    copy(ROOT/name,Path('source/original')/name)
modern=read(ROOT/'artifacts/candidates/cpp-accounting-layout-control/build.json')
assert modern['binary_sha256']==matrix['modern_sha256']
copy(ROOT/'artifacts/candidates/cpp-accounting-layout-control/build.json','provenance/modern-build.json')
for name, expected in modern['source_sha256'].items():
    p=ROOT/'artifacts/candidates/cpp-accounting-layout-control'/name
    assert sha(p)==expected
    copy(p,Path('source/modern')/name)
rust=read(ROOT/'artifacts/candidates/observer-quantum/build.json')
assert rust['binary_sha256']==matrix['rust_sha256']
for name in ('build.json','runtime.patch'):
    copy(ROOT/'artifacts/candidates/observer-quantum'/name,Path('provenance')/('rust-'+name))
with tempfile.TemporaryDirectory(prefix='war-threeway-rust-') as tmp:
    root=Path(tmp)
    for name in rust['sources']:
        p=root/name;p.parent.mkdir(parents=True,exist_ok=True)
        p.write_bytes(subprocess.check_output(['git','show',f'{rust["base_revision"]}:{name}'],cwd=ROOT))
    subprocess.run(['git','apply',str(ROOT/'artifacts/candidates/observer-quantum/runtime.patch')],cwd=root,check=True)
    for name, expected in rust['sources'].items():
        assert sha(root/name)==expected,(name,sha(root/name),expected)
        copy(root/name,Path('source/rust')/name)
# No binaries are tracked. These paths identify the exact local frozen artifacts.
put('reproduce.json',json.dumps({'command':['python3','scripts/compare-cpp.py','--relay',matrix['args']['relay'],'--modern',matrix['args']['modern'],'--client',matrix['args']['client'],'--client-source-root',matrix['args']['client_source_root'],'--rebuild-cpp','--workers','4','--rooms','8192','--pairs','64','--client-workers','8','--server-cpus','0-3','--client-cpus','4-11','--seconds','6','--repetitions','6','--cases','private','query','fanout','--output','artifacts/threeway-current-rerun.json'],'scope':matrix['scope'],'binary_hashes':{kind:matrix[kind+'_sha256'] for kind in ('cpp','rust','modern')},'client_sha256':matrix['client_sha256'],'note':'Restore archived scripts to repository-relative paths. Use recorded local frozen ELFs or rebuild corresponding sources; another toolchain may change hashes. The archived client sources are a historical snapshot, not a fresh source-to-binary reproducibility claim. Run sequentially without other benchmarks or builds.'},indent=2)+'\n')
# README is written by the analysis/report step before finalizing the manifest.
print(DEST)
