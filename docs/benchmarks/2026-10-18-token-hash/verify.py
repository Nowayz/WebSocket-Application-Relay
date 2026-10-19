"""Check rejected token hash experiment receipts offline."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
summary = json.loads((ROOT / 'summary.json').read_text())
assert summary['selected'] is False
identities = {}
for row in summary['rows']:
    d = json.loads(gzip.decompress((ROOT / 'raw' / row['receipt']).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code'] == d['client_exit_code'] == 0
    assert d['server_sha256'] == row['server_sha256']
    identities.setdefault(row['variant'], d['server_sha256'])
    assert identities[row['variant']] == d['server_sha256']
    m = d['mixed']
    n = m['healthy']['verified'] + m['fanout']['verified']
    assert abs(row['cpu_seconds_per_million_deliveries'] - m['server_cpu_seconds'] / n * 1e6) < 1e-12
    end = d['events'][-1]
    assert end['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert end['metrics'][key] == 0, key
    assert end['metrics']['war_channels'] == 1
    print(row['receipt'], 'verified')
assert identities['control'] != identities['candidate']
