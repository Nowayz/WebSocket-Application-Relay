import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert meta['runtime_changed'] is False
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
parent = json.loads((root.parent / 'provenance.json').read_text())
assert meta['production_server_sha256'] == parent['selected_source_sha256']['src/server.rs']
s = (root / 'fairness_tests.rs').read_text()
s = s[s.index('fn readable_upgrades_cannot_bypass_deadlines_or_shutdown_behind_an_expiry_batch'):]
for expression in ['for stopping in [false, true]', 'worker.schedule_timers(now)', 'event.is_readable()', 'worker.schedule(token)', 'connection.drive(', 'Drive::Drop', 'Stage::Upgrade(_)', 'u64::from(!stopping)', 'io::ErrorKind::ConnectionReset']:
    assert expression in s, expression
assert '122 passed; 0 failed' in (root / 'tests.log').read_text()
assert 'Compiling war-relay' in (root / 'tests.log').read_text()
assert 'Finished' in (root / 'clippy.log').read_text()
print('Queued-timer readable-upgrade and shutdown coverage verified; runtime unchanged.')
