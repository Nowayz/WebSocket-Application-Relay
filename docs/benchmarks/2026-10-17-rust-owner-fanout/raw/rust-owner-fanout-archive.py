"""Freeze all iterations and measured evidence; ELF/perf data stay local."""
import difflib,lzma,hashlib,json,shutil,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
DEST=ROOT/'docs/benchmarks/2026-10-17-rust-owner-fanout'
DEP=ROOT/'docs/benchmarks/2026-10-17-threeway-current'
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def read(p):return json.loads(Path(p).read_text())
def copy(p,q):
 t=DEST/q;t.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(p,t)
assert not DEST.exists()
DEST.mkdir(parents=True)
contents={}
for pattern in ['rust-owner-fanout-*','rust-fanout-profile*','freeze-rust-fanout.py']:
 for p in sorted((ROOT/'artifacts').glob(pattern)):
  if not p.is_file() or 'preexisting' in p.name or 'preserved' in p.name:continue
  if p.suffix not in ['.json','.py','.log','.txt','.png','.pdf','.csv','.patch']:continue
  if p.suffix=='.csv':
   t=DEST/'raw'/(p.name+'.xz');t.parent.mkdir(exist_ok=True);t.write_bytes(lzma.compress(p.read_bytes(),preset=6));contents[str(t.relative_to(DEST))]=sha(p)
  else:copy(p,Path('raw')/p.name)
versions=['rust-owner-fanout-v1','rust-owner-fanout','rust-owner-fanout-all','rust-owner-fanout-isolated','rust-owner-fanout-explicit']
base=read(ROOT/'artifacts/candidates/observer-quantum/build.json')
copy(ROOT/'artifacts/candidates/observer-quantum/build.json','provenance/baseline-build.json')
for version in versions:
 p=ROOT/'artifacts/candidates'/version;record=read(p/'build.json');assert sha(p/'war-relay')==record['binary_sha256']
 copy(p/'build.json',Path('source')/version/'build.json')
 patch=[]
 for name,expected in record['sources'].items():
  assert sha(p/name)==expected,(version,name)
  if base['sources'][name]!=expected:
   copy(p/name,Path('source')/version/name)
   old=(DEP/'source/rust'/name).read_text().splitlines(keepends=True);new=(p/name).read_text().splitlines(keepends=True)
   patch.extend(difflib.unified_diff(old,new,fromfile='a/'+name,tofile='b/'+name))
 for name in ['Cargo.toml','Cargo.lock']:copy(p/name,Path('source')/version/name)
 (DEST/'source'/version/'runtime.patch').write_text(''.join(patch))
for name in ['compare-cpp.py','benchmark.py','inspection.py','reference.py','input-corpus.py','fragment-fairness.py']:
 p=ROOT/'scripts'/name
 if p.exists():copy(p,Path('source/scripts')/name)
(DEST/'dependency.json').write_text(json.dumps({'path':'../2026-10-17-threeway-current','manifest_sha256':sha(DEP/'manifest.json'),'purpose':'Unchanged selected Rust source/build and client provenance. Changed candidate files are stored in this archive; baseline files resolve from dependency/source/rust.'},indent=2)+'\n')
(DEST/'compressed-content.json').write_text(json.dumps(contents,indent=2)+'\n')
print(DEST)
