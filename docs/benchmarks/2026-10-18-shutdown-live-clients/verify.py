import gzip, hashlib, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()
source=json.loads((root/'source.json').read_text())
assert source['changed_files']==['src/server/shutdown_tests.rs']
assert source['source_sha256']['src/server/shutdown_tests.rs']==hashlib.sha256((root/'raw/shutdown_tests.rs').read_bytes()).hexdigest()
before=read('shutdown-live-clients-focused.log')
assert 'FAILED' in before and 'began.elapsed() >= Duration::from_secs(1)' in before
assert '1 passed; 0 failed' in read('shutdown-live-clients-corrected.log')
all_tests=read('shutdown-live-clients-all-tests.log')
assert '92 passed; 0 failed' in all_tests
assert 'shutdown_releases_blocked_output_and_pending_upgrades_with_live_clients ... ok' in all_tests
clippy=read('shutdown-live-clients-clippy.log')
assert 'Finished' in clippy and 'error:' not in clippy
print('Clean source identity, corrected fixture assumption, integrated live-client shutdown, 92 passing library tests and Clippy verified.')
