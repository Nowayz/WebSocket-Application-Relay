"""Verify rejected buffered-read screen and normalized CPU figures."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected,name = line.split('  ',1)
    assert hashlib.sha256((ROOT/name).read_bytes()).hexdigest() == expected,name
s=json.loads((ROOT/'summary.json').read_text())
b=json.loads((ROOT/'builds.json').read_text())
assert s['selected'] is False
for row in s['rows']:
    d=json.loads(gzip.decompress((ROOT/'raw'/row['file']).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code'] == d['client_exit_code'] == 0
    assert d['client_sha256'] == b['variants'][row['variant']]['binary_sha256']
    m=d['mixed'];n=m['fanout']['verified']
    for key,value in [('deliveries_per_second',n/m['elapsed_seconds']),
                      ('client_cpu_seconds_per_million',m['client_cpu_seconds']/n*1e6),
                      ('server_cpu_seconds_per_million',m['server_cpu_seconds']/n*1e6)]:
        assert abs(row[key]-value)<1e-10
    assert row['fanout_max_ms'] == m['fanout']['max_ms']
    end=d['events'][-1]
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert end['metrics'][k] == 0,k
print('Checksums, client binary identities, normalization and retirement verified.')
