"""Verify observed scheduling capacities and fixture retirement offline."""
import gzip
import hashlib
import json
import re
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
summary = json.loads((ROOT / 'summary.json').read_text())
assert summary['selected'] is False
for row in summary['rows']:
    d = json.loads(gzip.decompress((ROOT / 'raw' / (row['case'] + '.json.gz')).read_bytes()))
    assert d['passed'] and d['client_exit_code'] == d['server_exit_code'] == 0
    assert d['server_sha256'] == row['server_sha256']
    pairs = [[int(a),int(b)] for a,b in re.findall(r'scheduling-empty ready=(\d+) pending=(\d+)', d['server_log'])]
    assert pairs == row['empty_worker_capacities']
    assert max(sum(pair)*summary['token_bytes'] for pair in pairs) == row['max_reported_bytes_per_worker']
    done = [e for e in d['events'] if e['event'] == 'done']
    assert len(done) == (4 if row['case'] == 'waves' else 1)
    for end in done:
        for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
                  'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
            assert end['metrics'][k] == 0, k
        if row['case'] == 'waves':
            assert end['established_server_sockets'] == 0
    print(row['case'], 'verified')
