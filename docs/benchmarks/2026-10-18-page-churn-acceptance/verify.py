"""Verify short HTTP/churn acceptance receipts offline."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
for row in json.loads((ROOT / 'summary.json').read_text()):
    d = json.loads(gzip.decompress((ROOT / 'raw' / (row['case'] + '.json.gz')).read_bytes()))
    assert d['passed'] and not d['profile_diagnostic'] and not d['server_forced_kill']
    assert d['server_exit_code'] == d['client_exit_code'] == 0
    for k in ['server_sha256', 'client_sha256', 'runner_sha256']:
        assert d[k] == row[k]
    assert d['args']['page_size'] == row['page_size']
    assert d['args']['rooms'] == row['rooms']
    assert row['counts'] == {k:v['verified'] for k,v in d['mixed'].items() if isinstance(v,dict) and 'verified' in v}
    assert row['counts']['inspection'] == 400 and row['counts']['churn'] > 0
    end = d['events'][-1]
    assert end['event'] == 'done'
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert end['metrics'][k] == 0, k
    assert end['metrics']['war_channels'] == 1
    print(row['case'], 'verified')
