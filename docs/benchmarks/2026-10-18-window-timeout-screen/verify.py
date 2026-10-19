"""Audit the rejected window-timeout screen without running relay traffic."""
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


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())


def receipt(name):
    return json.loads(decoded(name + '.json'))


def retired(metrics):
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                'war_routing_copied_bytes_total']:
        assert metrics[key] == 0, key
    for key, value in metrics.items():
        if any(s in key for s in ['errors_total', 'rejected_total', 'slow_consumers_total',
                                 'timeouts_total', 'aborted_writes_total', 'close_failures_total',
                                 'transport_disconnects_total', 'global_pressure_total']):
            assert value == 0, key


build = json.loads((ROOT / 'builds.json').read_text())
summary = json.loads((ROOT / 'summary.json').read_text())
assert not build['selected']
for row in summary['short']:
    x = receipt(f"window-timeout-short-{row['trial']}-{row['variant']}")
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code'] == x['server_exit_code'] == 0 and not x['server_forced_kill']
    assert x['client_sha256'] == build[row['variant'] + '_client_sha256']
    assert x['server_sha256'] == build['server_sha256']
    m = x['mixed']
    assert m['healthy']['verified'] == 0
    for process in ['client', 'server']:
        assert row[process + '_cpu_seconds_per_million'] == m[process + '_cpu_seconds'] / m['fanout']['verified'] * 1e6
    done = next(e for e in x['events'] if e['event'] == 'done')
    assert done['details']['passed']
    retired(done['metrics'])
for row in summary['population']:
    name = 'window-timeout-100k-' + row['variant']
    x = receipt(name)
    assert x['passed'] and x['exit_codes'] == [0, 0, 0] and not x['args']['profile']
    assert x['sha256']['server'] == build['server_sha256']
    assert x['sha256']['sidecar'] == build[row['variant'] + '_client_sha256']
    assert x['fanout_windows'] == {'completed': 96, 'offered': 96}
    assert x['fanout_offered_load_met'] and not x['generator_lag']['exceeded_one_period']
    assert row['client_cpu_seconds'] == x['client_after']['sidecar']['cpu_seconds'] - x['client_before']['sidecar']['cpu_seconds']
    assert row['server_cpu_seconds'] == x['server_cpu_seconds']
    for suffix, count, samples, field, scale, reported in [
            ('.csv', 100000, 24, 'max_ms', 1, x['latency']['max_ms']),
            ('-fanout.csv', 8192, 384, 'max_ns', 1e6, x['fanout_exact_ms']['max_ms'])]:
        raw = decoded(name + suffix)
        assert hashlib.sha256(raw).hexdigest() == x['csv_sha256' if suffix == '.csv' else 'fanout_csv_sha256']
        rows = list(csv.DictReader(io.StringIO(raw.decode())))
        assert len(rows) == len({r['id'] for r in rows}) == count
        assert all(int(r['samples']) == samples for r in rows)
        values = sorted(float(r[field]) / scale for r in rows)
        for key, p in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]:
            assert reported[key] == values[(count - 1) * p // 100]
    assert hashlib.sha256(decoded(name + '.windows.json')).hexdigest() == x['latency_windows_sha256']
    retired(x['final_metrics'])
    assert row['private_maxima_ms'] == x['latency']['max_ms']
    assert row['fanout_maxima_ms'] == x['fanout_exact_ms']['max_ms']
    assert row['private_intended_maxima_ms'] == x['latency']['schedule_max_ms']
    assert row['fanout_intended_maxima_ms'] == x['fanout_exact_ms']['schedule_max_ms']
assert '8 passed; 0 failed' in decoded('window-timeout-tests.log').decode()
print('Checksums, frozen identities, four ordinary trials, both 100k populations, offered load and retirement verified.')
