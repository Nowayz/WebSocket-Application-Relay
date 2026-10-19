"""Verify diagnostic clock counts and final resource gates."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
s = json.loads((ROOT / 'summary.json').read_text())
assert s['selected'] is False
assert len(json.loads((ROOT / 'sites.json').read_text())) == 14
for row in s['rows']:
    d = json.loads(gzip.decompress((ROOT / 'raw' / (row['case'] + '.json.gz')).read_bytes()))
    assert d['passed'] and d['server_exit_code'] == d['client_exit_code'] == 0
    assert not d['server_forced_kill'] and d['server_sha256'] == s['server_sha256']
    a, b, end = d['events']
    assert [a['event'], b['event'], end['event']] == ['ready', 'measured', 'done']
    for k, v in row['counts'].items():
        assert b['metrics'][k] - a['metrics'][k] == v
    assert row['counts']['war_clock_probe_untracked_total'] == 0
    for k in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
              'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert end['metrics'][k] == 0, k
    assert end['metrics']['war_channels'] == 1
    print(row['case'], 'verified')
