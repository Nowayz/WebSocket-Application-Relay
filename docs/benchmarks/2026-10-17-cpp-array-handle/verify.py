from pathlib import Path
import gzip,hashlib,json,subprocess,sys
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());sha=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert sha((p/name).read_bytes())==want,name
for name,r in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert sha(data)==r['sha256'] and len(data)==r['bytes'],name
d=p.parent/'2026-10-17-cpp-placement';assert sha((d/'manifest.json').read_bytes())==m['dependency_manifest_sha256']
subprocess.run([sys.executable,str(d/'verify.py')],check=True)
f=p/'cpp-array-handle-placement.json.gz';r=json.loads(gzip.decompress(f.read_bytes()) if f.exists() else (p/'cpp-array-handle-placement.json').read_bytes())
assert r['passed'] and len(r['runs'])==24 and all(row['passed'] and row['server_exit_code']==0 for row in r['runs'])
assert r['runner_sha256']==sha((p/'placement-compare.py').read_bytes())
b=json.loads((p/'source/build.json').read_text());assert b['binary_sha256']==r['server_sha256']['handle']
for name,want in b['source_sha256'].items():assert sha((p/'source'/name).read_bytes())==want,name
assert r['client_sha256']==json.loads((d/'client-build.json').read_text())['client_sha256']
print(json.dumps({'passed':True,'files':len(m['files']),'ordinary_trials':24,'dependency_verified':True}))
