#!/usr/bin/env python3
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
