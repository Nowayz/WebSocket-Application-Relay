import gzip,hashlib,json
from pathlib import Path
r=Path(__file__).resolve().parent
m=json.loads((r/'provenance.json').read_text())
for name,digest in m['files_sha256'].items():
    assert hashlib.sha256((r/name).read_bytes()).hexdigest()==digest,name
assert m['runtime_changed'] is False
assert '127 passed; 0 failed' in (r/'tests.log').read_text()
assert '1 passed; 0 failed' in (r/'focused.log').read_text()
mutant=(r/'mutant.log').read_text();assert '0 passed; 1 failed' in mutant and 'Elapsed(())' in mutant and '3.00s' in mutant
clippy=(r/'clippy.log').read_text();assert 'Finished `dev` profile' in clippy and 'error:' not in clippy
old=gzip.decompress((r/'original-server.rs.gz').read_bytes()).decode()
a=old.index('impl Drop for StopOnDrop');b=old.index('\nstruct Worker',a)
expected=old[:a]+old[a:b].replace('for signal in &self.signals {\n            signal.stop();\n        }','// Mutation: do not signal or wake native workers on cancellation.')+old[b:]
assert expected==gzip.decompress((r/'mutant-server.rs.gz').read_bytes()).decode()
patch=(r/'selected.patch').read_text();assert patch.count('diff --git')==1 and 'src/server/shutdown_tests.rs' in patch
for line in patch.splitlines():
    assert not line.startswith('-') or line.startswith('---'),line
print('Serve cancellation verified: test-only addition, 127 passing tests, successful Clippy and sensitive three-second mutation failure.')
