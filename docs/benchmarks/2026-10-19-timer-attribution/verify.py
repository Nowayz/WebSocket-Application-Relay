import gzip
import hashlib
import json
import re
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert meta['diagnostic_only'] and meta['selected_runtime_unchanged']
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
assert build['source_sha256']['src/server/timers.rs'] == hashlib.sha256((root / 'diagnostic-timers.rs').read_bytes()).hexdigest()
assert (root / 'tested-timers.rs').read_bytes().startswith((root / 'diagnostic-timers.rs').read_bytes())
assert '3 passed; 0 failed' in (root / 'tests.log').read_text()
summary = json.loads((root / 'summary.json').read_text())
assert summary['passed'] and summary['diagnostic_only'] and len(summary['runs']) == 4
assert {(r['users'], r['ping_interval_secs']) for r in summary['runs']} == {(8192, 0), (8192, 1), (8192, 15), (100000, 15)}
server_hashes = set()
for row in summary['runs']:
    raw = gzip.decompress((root / (row['label'] + '.json.gz')).read_bytes())
    assert hashlib.sha256(raw).hexdigest() == row['receipt_sha256']
    receipt = json.loads(raw)
    assert receipt['passed'] and receipt['client_exit_code'] == receipt['server_exit_code'] == 0
    server_hashes.add(receipt['server_sha256'])
    assert receipt['args']['seconds'] == 2
    assert receipt['measurement']['application_deliveries'] == row['users'] * 8
    assert all(x['verified_after_measurement'] for x in receipt['thread_affinity'])
    counts = [(json.loads(m[0]), int(m[1]), int(m[2])) for m in re.findall(r'timer-attribution: (\[[^\n]+?\]) peak=(\d+) remaining=(\d+)', receipt['server_log'])]
    assert len(counts) == row['diagnostic_instances']
    assert all(remaining == 0 for _, _, remaining in counts)
    assert [sum(c[i] for c, _, _ in counts) for i in range(6)] == row['counts']
    assert sum(row['counts']) == row['setter_calls']
    assert row['unchanged_percent'] == row['counts'][3] / row['setter_calls'] * 100
    for event in receipt['events']:
        for key, value in event['exact_counter_checks'].items():
            assert event['metrics'][key] == value
    done = next(e for e in receipt['events'] if e['event'] == 'done')
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0
    assert done['server']['fd_count'] == receipt['baseline']['server']['fd_count']
    print(f"{row['label']}: {row['setter_calls']:,} setter calls; {row['unchanged_percent']:.2f}% unchanged")
assert len(server_hashes) == 1
print('Four diagnostic runs verified; eight traffic seconds, unchanged selected runtime.')
