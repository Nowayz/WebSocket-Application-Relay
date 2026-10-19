import gzip
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
source = (root / 'join-pressure.py').read_text()
assert "os.path.lexists(path)" in source
assert "with args.output.open('x')" in source and "with log_path.open('x')" in source
assert source.index("record['server_sha256'] = hashlib") < source.index('server = subprocess.Popen')
assert "record['server_exit_code'] = None" in source
assert 'FAILED (errors=7)' in (root / 'before.log').read_text()
assert 'Ran 2 tests' in (root / 'tests.log').read_text() and '\nOK\n' in (root / 'tests.log').read_text()
counterfactual = json.loads((root / 'counterfactual.json').read_text())
assert counterfactual['original_truncated_existing_log'] and counterfactual['original_failed_to_save_receipt']
cli = json.loads((root / 'cli.json').read_text())
assert cli['passed'] and {r['mode'] for r in cli['cases']} == {'missing', 'permission', 'interpreter'}
for case in cli['cases']:
    receipt = case['receipt']
    assert case['cli_exit_code'] == 1 and not receipt['passed']
    assert receipt['server_exit_code'] is None and receipt['error']
    assert receipt['script_sha256'] == meta['files_sha256']['join-pressure.py']
    assert not receipt.get('server_forced_kill')
real = json.loads(gzip.decompress((root / 'real.json.gz').read_bytes()))
assert real['passed'] and real['connections'] == 32 and real['waves'] == 1
assert real['server_exit_code'] == 0 and not real.get('server_forced_kill')
assert real['healthy_verified'] == 226696
assert real['script_sha256'] == meta['files_sha256']['join-pressure.py']
for name, value in real['exact_checks'].items():
    assert real['final_metrics'][name] == value
for name in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total']:
    assert real['final_metrics'][name] == 0
print('Prejoin output evidence verified: seven fast cases, three CLI failures, one two-second real-socket wave.')
