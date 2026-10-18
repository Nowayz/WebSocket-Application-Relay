"""Freeze the completed study; run only after all benchmarks and reports finish."""
from pathlib import Path
import hashlib,json,lzma,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-18-mixed-100k-fairness'
assert not out.exists()
(out/'raw').mkdir(parents=True);(out/'source').mkdir()
compressed={}
for path in sorted((root/'artifacts').glob('mixed100k-*')):
 if not path.is_file() or path.suffix not in {'.json','.log','.txt','.csv','.py','.err','.png','.pdf'}:continue
 if path.suffix=='.csv':
  data=path.read_bytes();dest=out/'raw'/(path.name+'.xz');dest.write_bytes(lzma.compress(data,preset=6));compressed[dest.name]={'decoded_sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}
 else:shutil.copy2(path,out/'raw'/path.name)
for label,directory in [('after','mixed-capacity-final'),('client','mixed-capacity-client-final'),('before','rust-owner-fanout-explicit'),('index-experiment','mixed-capacity-index'),('output-experiment','mixed-capacity-output'),('quantum256-experiment','mixed-capacity-continuation'),('quantum1024-unaccounted-experiment','mixed-capacity-quantum1024'),('quantum4096-experiment','mixed-capacity-quantum4096')]:
 source=root/'artifacts/candidates'/directory;dest=out/'source'/label;dest.mkdir()
 for name in ['src','examples']:
  if (source/name).exists():shutil.copytree(source/name,dest/name)
 for name in ['Cargo.toml','Cargo.lock','build.json']:
  if (source/name).exists():shutil.copy2(source/name,dest/name)
shutil.copytree(root/'artifacts/candidates/mixed-capacity-pressure-client',out/'source/pressure-client',ignore=shutil.ignore_patterns('stress'))
# One unchanged transport vendor tree is shared by all Rust snapshots.
shutil.copytree(root/'vendor',out/'source/vendor',ignore=shutil.ignore_patterns('.git','target'))
source=root/'artifacts/candidates/cpp-accounting-layout-control'
shutil.copytree(source/'cpp',out/'source/cpp',ignore=shutil.ignore_patterns('build','target','.git'))
shutil.copy2(source/'build.json',out/'source/cpp-build.json')
for name in ['scripts/mixed-capacity.py','scripts/capacity.py','scripts/capacity_pressure.py','scripts/input-corpus.py','scripts/benchmark.py','scripts/fragment-fairness.py']:
 dest=out/'source'/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(root/name,dest)
shutil.copytree(root/'artifacts/candidates/latency-timeline-client',out/'source/capacity-client',ignore=shutil.ignore_patterns('capacity','target','.git'))
(out/'compressed.json').write_text(json.dumps(compressed,indent=2)+'\n')
deps={str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [root/'docs/benchmarks/2026-10-17-rust-owner-fanout/manifest.json',root/'docs/benchmarks/2026-10-17-threeway-current/manifest.json']}
(out/'dependencies.json').write_text(json.dumps(deps,indent=2)+'\n')
subprocess.run(['git','diff','--no-index','--',str(out/'source/before/src'),str(out/'source/after/src')],stdout=(out/'source/runtime.patch').open('w'))
shutil.copy2(root/'artifacts/mixed100k-report.md',out/'README.md')
shutil.copy2(root/'artifacts/mixed100k-verify.py',out/'verify.py')
files={str(p.relative_to(out)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.rglob('*')) if p.is_file()}
(out/'manifest.json').write_text(json.dumps(files,indent=2)+'\n')
print(json.dumps({'files':len(files),'bytes':sum(p.stat().st_size for p in out.rglob('*') if p.is_file())}))
