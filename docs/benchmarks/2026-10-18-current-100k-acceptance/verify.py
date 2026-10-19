"""Verify full-population finite acceptance without starting relay processes."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name

def decoded(suffix):
    return gzip.decompress((ROOT / 'raw' / ('current-100k-mixed' + suffix + '.gz')).read_bytes())

x = json.loads(decoded('.json'))
build = json.loads((ROOT / 'raw/build.json').read_text())
assert x['passed'] and not x['args']['profile'] and x['exit_codes'] == [0,0,0]
assert x['sha256']['server'] == build['binary_sha256']
assert hashlib.sha256((ROOT / 'raw/mixed-capacity.py').read_bytes()).hexdigest() == x['sha256']['runner']
assert x['fanout_windows'] == {'completed':96,'offered':96} and x['fanout_offered_load_met']
assert len(x['client_thread_affinity']) == 16
for suffix, field in [('.csv','csv_sha256'),('-fanout.csv','fanout_csv_sha256'),('.windows.json','latency_windows_sha256')]:
    assert hashlib.sha256(decoded(suffix)).hexdigest() == x[field]
assert x['latency_windows_sha256'] == x['latency_stages_windows_sha256']
for suffix, count, samples, field, scale, reported in [
    ('.csv',100000,24,'max_ms',1,x['latency']['max_ms']),
    ('-fanout.csv',8192,384,'max_ns',1000000,x['fanout_exact_ms']['max_ms'])]:
    rows = list(csv.DictReader(io.StringIO(decoded(suffix).decode())))
    assert len(rows) == len({r['id'] for r in rows}) == count
    assert all(int(r['samples']) == samples for r in rows)
    values = sorted(float(r[field])/scale for r in rows)
    for key, p in [('median',50),('p95',95),('p99',99),('max',100)]:
        assert values[(count-1)*p//100] == reported[key]
    if suffix == '.csv':
        assert all(float(r['p99_ms']) == float(r['max_ms']) for r in rows)
        assert sum(int(r['samples']) for r in rows) == 2400000
    else:
        assert sum(int(r['samples']) for r in rows) == 3145728
assert x['latency_sampling']['all_user_p99_equal_observed_max']
assert not x['generator_lag']['exceeded_one_period']
windows = json.loads(decoded('.windows.json'))['windows']
assert len(windows) == 6
assert all(w[k]['samples'] == 400000 for w in windows for k in ['pre_send_delay','send_to_receive','intended_to_receive'])
for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
          'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
    assert x['final_metrics'][k] == 0, k
for k,v in x['final_metrics'].items():
    if any(word in k for word in ['errors_total','rejected_total','slow_consumers_total','timeouts_total',
                                  'aborted_writes_total','close_failures_total','transport_disconnects_total','global_pressure_total']):
        assert v == 0, k
print('Checksums, decoded hashes, 108192 user rows, offered load, population statistics and final gauges verified.')
