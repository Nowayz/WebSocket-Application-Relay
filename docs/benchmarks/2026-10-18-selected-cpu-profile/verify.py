"""Verify portable CPU attribution receipts without rerunning traffic."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
for item in json.loads((ROOT / 'profiles.json').read_text()):
    name = item['case']
    d = json.loads(gzip.decompress((ROOT / 'raw' / (name + '.json.gz')).read_bytes()))
    assert d['passed'] and d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code'] == d['client_exit_code'] == 0
    assert d['perf_exit_code'] in (0, -2)
    assert d['server_sha256'] == item['server_sha256']
    end = d['events'][-1]
    assert end['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert end['metrics'][key] == 0, key
    assert end['metrics']['war_channels'] == 1
    assert 'Total Lost Samples: 0' in (ROOT / 'raw' / (name + '.report.txt')).read_text()
    print(name, 'verified')
