"""Audit snapshot builder acceptance without compiling or starting a relay."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes()).decode()


assert 'Ran 5 tests' in decoded('snapshot-builder-tests.log')
assert 'OK' in decoded('snapshot-builder-tests.log')
records = {}
for kind, target_kind, name in [('native', 'bin', 'war-relay'), ('inspection', 'example', 'inspection')]:
    record = json.loads(decoded(kind + '-build.json'))
    assert record['passed'] and record['cargo_exit_code'] == 0
    artifact = record['cargo_artifact']
    assert artifact['fresh'] is False and artifact['target']['name'] == name
    assert target_kind in artifact['target']['kind']
    assert '--locked' in record['command'] and '--release' in record['command']
    messages = []
    for line in decoded(kind + '-build.log').splitlines():
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    assert artifact in messages
    records[kind] = record
assert records['native']['source_sha256'] == records['inspection']['source_sha256']
assert len(records['native']['source_sha256']) == 110
corpus = json.loads(decoded('snapshot-builder-corpus.json'))['case']
assert corpus['passed'] and corpus['corpus_cases'] == 776 and corpus['scheduled_exchanges'] == 2311
assert corpus['server_sha256'] == records['native']['binary_sha256']
smoke = json.loads(decoded('snapshot-builder-smoke.json'))
assert smoke['passed'] and smoke['client_exit_code'] == smoke['server_exit_code'] == 0
assert not smoke['server_forced_kill']
assert smoke['server_sha256'] == records['native']['binary_sha256']
assert smoke['client_sha256'] == records['inspection']['binary_sha256']
done = next(e for e in smoke['events'] if e['event'] == 'done')
assert done['details']['passed']
for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
            'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
            'war_routing_copied_bytes_total']:
    assert done['metrics'][key] == 0, key
print('Checksums, five real-Cargo regressions, forced native/example builds, corpus and functional retirement verified.')
