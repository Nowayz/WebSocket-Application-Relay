"""Verify unselected inline-integer frame screen offline."""
import gzip
import hashlib
import json
import statistics
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
s = json.loads((ROOT / 'summary.json').read_text())
build = json.loads((ROOT / 'builds.json').read_text())
assert s['selected'] is False and len(s['component']) == 8 and len(s['mixed']) == 4
for item in s['component']:
    rows = [json.loads(line) for line in (ROOT / 'raw' / item['file']).read_text().splitlines()]
    assert len(rows) == 8
    computed = []
    for row in rows:
        assert row['frame_bytes'] == 48 and len(row['ns_per_operation']) == 7
        if item['variant'] != 'control':
            assert row['payload_bytes'] == 0
        computed.append({**{k:v for k,v in row.items() if k != 'ns_per_operation'},
                         'median_ns':statistics.median(row['ns_per_operation'])})
    assert computed == item['rows']
for row in s['mixed']:
    d = json.loads(gzip.decompress((ROOT / 'raw' / row['file']).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code'] == d['client_exit_code'] == 0
    assert d['server_sha256'] == row['server_sha256'] == build['server_binaries'][row['variant']]
    m = d['mixed']
    n = m['healthy']['verified'] + m['fanout']['verified']
    assert abs(m['server_cpu_seconds']/n*1e6 - row['cpu_seconds_per_million_deliveries']) < 1e-12
    end = d['events'][-1]
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert end['metrics'][k] == 0, k
assert '5 passed; 0 failed' in (ROOT / 'raw/inline-u64-frame-tests.log').read_text()
print('Checksums, component medians/layouts, mixed CPU normalization and retirement verified.')
