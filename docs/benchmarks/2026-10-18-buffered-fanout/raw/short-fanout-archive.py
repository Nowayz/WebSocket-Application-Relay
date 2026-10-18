"""Freeze this finite experiment, including rejected candidates and decoded CSV hashes."""
from pathlib import Path
import hashlib,json,lzma,shutil,subprocess,tarfile,io
root=Path.cwd();out=root/'docs/benchmarks/2026-10-18-buffered-fanout'
assert not out.exists();out.mkdir(parents=True)
manifest={};decoded={}
def save(source,target):
 target.parent.mkdir(parents=True,exist_ok=True)
 data=source.read_bytes()
 if source.suffix=='.csv':
  decoded[str(target.relative_to(out))]={'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)}
  target=target.with_suffix('.csv.xz');target.write_bytes(lzma.compress(data,preset=1))
 else:shutil.copy2(source,target)
for source in sorted((root/'artifacts').glob('short-fanout-*')):
 if source.is_file():save(source,out/'raw'/source.name)
for name in ['short-fanout-ready64','short-fanout-priority64','short-fanout-batch4','short-fanout-batch-final','short-fanout-batch-selected','short-fanout-policy-final','short-fanout-batch2-final','short-fanout-ready64-batch','short-fanout-client','short-fanout-client-v2','short-fanout-client-retained','latency-timeline-client','mixed-capacity-final']:
 folder=root/'artifacts/candidates'/name
 if not folder.exists():continue
 for source in folder.rglob('*'):
  if source.is_file():save(source,out/'sources'/name/source.relative_to(folder))
# Complete the baseline source tree with its exact selected dependency sources.
archive=subprocess.check_output(['git','archive','279c3c1836c806ecf3343a2f00a89fac7cead022','vendor'])
with tarfile.open(fileobj=io.BytesIO(archive)) as entries:entries.extractall(out/'sources/mixed-capacity-final',filter='data')
# The timed inspection executable was built before relay changes. Reconstruct
# that exact source context from the saved original observer-index overlay.
client=out/'sources/inspection-client-v2-project'
shutil.copytree(out/'sources/mixed-capacity-final',client,ignore=shutil.ignore_patterns('war-relay','build.json'))
for name in ['src/relay.rs','src/relay/listener_tests.rs']:
 source=root/'artifacts/short-fanout-preexisting'/name
 shutil.copyfile(source,client/name)
shutil.copyfile(root/'artifacts/candidates/short-fanout-client-v2/inspection.rs',client/'examples/inspection.rs')
shutil.copyfile(root/'artifacts/candidates/short-fanout-client-v2/fanout.rs',client/'examples/inspection/fanout.rs')
for name in ['scripts/mixed-capacity.py','scripts/input-corpus.py','scripts/fragment-fairness.py','scripts/benchmark.py','scripts/check-vendor.py']:
 save(root/name,out/name)
# Exact original capacity source context, not today's changed example.
archive=subprocess.check_output(['git','archive','708dc753efb48147258855444de83ed889995a31','Cargo.toml','Cargo.lock','src','examples','vendor'])
with tarfile.open(fileobj=io.BytesIO(archive)) as entries:entries.extractall(out/'sources/capacity-client-project',filter='data')
shutil.copyfile(root/'artifacts/candidates/latency-timeline-client/capacity.rs',out/'sources/capacity-client-project/examples/capacity.rs')
shutil.copyfile(root/'artifacts/short-fanout-report.md',out/'README.md')
shutil.copyfile(root/'artifacts/short-fanout-verify.py',out/'verify.py')
for path in out.rglob('*'):
 if path.is_file():manifest[str(path.relative_to(out))]=hashlib.sha256(path.read_bytes()).hexdigest()
(out/'manifest.json').write_text(json.dumps({'files':manifest,'decoded_csv':decoded},indent=2)+'\n')
print(json.dumps({'files':len(manifest),'bytes':sum(p.stat().st_size for p in out.rglob('*') if p.is_file())}))
