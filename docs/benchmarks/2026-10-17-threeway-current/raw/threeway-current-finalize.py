#!/usr/bin/env python3
import hashlib,json
from pathlib import Path
import subprocess,sys
ROOT=Path(__file__).resolve().parents[1]
DEST=ROOT/'docs/benchmarks/2026-10-17-threeway-current'
for name in ('analyze','plot','archive','report'):
 subprocess.run([sys.executable,str(ROOT/f'artifacts/threeway-current-{name}.py')],cwd=ROOT,check=True)
(DEST/'verify.py').write_text('''#!/usr/bin/env python3
"""Verify immutable archive bytes, trial provenance and selected source contracts."""
import hashlib,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def read(p):return json.loads((ROOT/p).read_text())
m=read('manifest.json')
for name,digest in m['files'].items():assert sha(ROOT/name)==digest,name
matrix=read('raw/threeway-current.json');summary=read('raw/threeway-current-summary.json')
assert matrix['completed'] and summary['completed'] and len(matrix['cases'])==54
assert summary['source_sha256']==sha(ROOT/'raw/threeway-current.json')
assert sha(ROOT/'source/scripts/compare-cpp.py')==matrix['runner_sha256']
assert sha(ROOT/'raw/threeway-current-continue.py')==matrix['continuation_runner_sha256']
for row in matrix['cases']:
 p=ROOT/'raw'/Path(row['trace']).name
 assert sha(p)==row['trace_sha256']
 d=json.loads(p.read_text())
 assert d['passed']==row['passed']
 assert d['server_sha256']==matrix[row['kind']+'_sha256']
 assert d['client_sha256']==matrix['client_sha256']
 assert d['runner_sha256']==matrix['runner_sha256']
for name,expected in read('provenance/rust-build.json')['sources'].items():
 assert sha(ROOT/'source/rust'/name)==expected,name
for name,expected in read('provenance/modern-build.json')['source_sha256'].items():
 assert sha(ROOT/'source/modern'/name)==expected,name
for name,expected in read('provenance/original-build.json')['upstream_sources'].items():
 assert sha(ROOT/'source/original'/name)==expected,name
assert sha(ROOT/'source/original/configured-relay.cpp')==read('provenance/original-build.json')['configured_source_sha256']
print(json.dumps({'archive_integrity_passed':True,'hashed_files':len(m['files']),'trials':54,'fully_passed_trials':summary['passed_trials'],'failed_trials':summary['failed_trials']}))
''')
manifest={'scope':'All bytes are retained, including preliminary harness probes and one ordinary query pacing failure. No failed case is silently retried or excluded. Integrity success is not a claim that every runtime case passed.', 'files':{str(p.relative_to(DEST)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(DEST.rglob('*')) if p.is_file() and p.name!='manifest.json'}}
(DEST/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
subprocess.run([sys.executable,str(DEST/'verify.py')],cwd=ROOT,check=True)
