from pathlib import Path
import gzip,hashlib,json,subprocess,sys
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());sha=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert sha((p/name).read_bytes())==want,name
for name,r in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert sha(data)==r['sha256'] and len(data)==r['bytes'],name
dependency=p.parent/'2026-10-17-cpp-single-allocation'
assert sha((dependency/'manifest.json').read_bytes())==m['source_dependency_manifest_sha256']
subprocess.run([sys.executable,str(dependency/'verify.py')],check=True)
case=p/'cpp-placement-controlled.json.gz'
r=json.loads(gzip.decompress(case.read_bytes()) if case.exists() else (p/'cpp-placement-controlled.json').read_bytes())
assert r['passed'] and len(r['runs'])==16 and all(row['passed'] for row in r['runs'])
assert r['runner_sha256']==sha((p/'cpp-placement-controlled-runner.py').read_bytes())
for label,kind in [('control','control'),('array','candidate')]:assert r['server_sha256'][label]==json.loads((dependency/'source'/kind/'build.json').read_text())['binary_sha256']
final=json.loads((p/'cpp-placement-final-smoke.json').read_text());assert final['passed'] and final['runner_sha256']==sha((p/'placement-compare.py').read_bytes())
b=json.loads((p/'client-build.json').read_text());assert b['client_sha256']==r['client_sha256']
for name,want in b['source_sha256'].items():assert sha((p/'client-source'/name).read_bytes())==want,name
assert sha((p/'client-library-worktree.patch').read_bytes())==b['library_worktree_patch_sha256']
for name in ['cpp-placement-smoke','cpp-placement-smoke2']:
 s=json.loads((p/(name+'.json')).read_text());assert s['runner_sha256']==sha((p/'cpp-placement-smoke-runner.py').read_bytes())
print(json.dumps({'passed':True,'files':len(m['files']),'ordinary_trials':16,'source_dependency_verified':True}))
