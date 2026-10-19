"""Verify stage attribution and profiled fixture without rerunning traffic."""
import gzip
import hashlib
import json
import statistics
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
ordinary = json.loads(gzip.decompress((ROOT / 'raw/ordinary.json.gz').read_bytes()))
diag = json.loads(gzip.decompress((ROOT / 'raw/diagnostic.json.gz').read_bytes()))
s = json.loads((ROOT / 'summary.json').read_text())
assert not ordinary['args']['profile'] and diag['args']['profile']
for x in [ordinary, diag]:
    assert x['passed'] and x['exit_codes'] == [0,0,0]
    assert x['fanout_windows'] == {'offered':96,'completed':96} and x['fanout_offered_load_met']
    for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
              'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert x['final_metrics'][k] == 0, k
for key in ['server','capacity_client','sidecar']:
    assert ordinary['sha256'][key] == diag['sha256'][key]
assert hashlib.sha256((ROOT / 'raw/mixed-fanout-attribution.py').read_bytes()).hexdigest() == diag['sha256']['runner']
w = next(e['details']['fanout_controller_timing']['windows'] for e in ordinary['sidecar_events'] if e['event'] == 'measured')
assert len(w) == s['windows'] == 96
for name,a,b in [('wake_lag','scheduled_ns','wake_ns'),('submission','wake_ns','sent_ns'),
                 ('post_submission','sent_ns','acknowledged_ns'),
                 ('completion_bookkeeping','acknowledged_ns','completed_ns'),
                 ('scheduled_to_complete','scheduled_ns','completed_ns')]:
    values = sorted((row[b]-row[a])/1e6 for row in w)
    assert min(values) >= 0
    assert s['stages'][name] == {'median_ms':statistics.median(values),'p99_ms':values[95*99//100],'max_ms':max(values)}
assert s['post_submission_over_40ms'] == sum(row['acknowledged_ns']-row['sent_ns'] > 40000000 for row in w)
for name in ['server','sidecar']:
    assert 'Total Lost Samples: 0' in (ROOT / 'raw' / f'current-100k-attribution-{name}-relative.report.txt').read_text()
print('Checksums, stage math, ordinary/diagnostic identities, offered load and retirement verified.')
