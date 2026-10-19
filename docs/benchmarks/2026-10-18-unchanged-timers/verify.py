"""Audit identical-deadline selection, corrected build identities and finite loads."""
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
    return json.loads(decoded(name))


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
assert hashlib.sha256((ROOT / 'raw/timers.rs').read_bytes()).hexdigest() == build['selected_timer_source_sha256']
assert all(build['ordinary_census_marker_absent'].values())
cache = receipt('timer-noop-cache-detection.json')
assert cache['stale_control_sha256'] == cache['census_sha256'] == build['census_binary_sha256']
assert not cache['ordinary_trials_started']
assert build['control_binary_sha256'] != cache['stale_control_sha256']
assert 'Compiling war-relay' in decoded('timer-noop-control-build.log').decode()
rows = json.loads((ROOT / 'summary.json').read_text())
assert len(rows) == 19
large = 0
for row in rows:
    name = row['receipt']
    x = receipt(name)
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code'] == x['server_exit_code'] == 0
    variant = ('census' if row['kind'] == 'diagnostic' else
               'control' if name.endswith('-control.json') else
               'selected' if name.endswith('-selected.json') else 'candidate')
    assert x['server_sha256'] == build[variant + '_binary_sha256']
    if variant != 'census':
        assert 'timer-census:' not in x['server_log']
    done = next(e for e in x['events'] if e['event'] == 'done')
    retired(done['metrics'])
    if row['kind'] == 'mixed':
        m = x['mixed']
        count = m['healthy']['verified'] + m['fanout']['verified']
        assert row['native_cpu_seconds_per_million'] == m['server_cpu_seconds'] / count * 1e6
        assert row['private_max_ms'] == m['healthy']['max_ms']
        assert row['fanout_max_ms'] == m['fanout']['max_ms']
    else:
        m = x['measurement']
        assert row['measurement'] == m
        assert m['application_deliveries'] == x['args']['users'] * 8
        assert done['server']['fd_count'] == x['baseline']['server']['fd_count']
        if x['args']['users'] == 100000:
            large += 1
            csv_name = name.removesuffix('.json') + '.csv'
            raw = decoded(csv_name)
            assert hashlib.sha256(raw).hexdigest() == build['csv_sha256'][csv_name]
            audience = list(csv.DictReader(io.StringIO(raw.decode())))
            assert len(audience) == len({r['id'] for r in audience}) == 100000
            assert all(int(r['samples']) == 8 for r in audience)
            assert max(float(r['max_ms']) for r in audience) == m['round_trip']['max_ms']
assert large == 6
census = receipt('timer-noop-census.json')
counts = [json.loads(line.split(':', 1)[1]) for line in census['server_log'].splitlines()
          if line.startswith('timer-census:')]
assert sum(r['set_calls'] for r in counts) == 178207
assert sum(r['same_some_calls'] for r in counts) == 150845
assert '92 passed; 0 failed' in decoded('timer-noop-root-tests.log').decode()
assert 'Finished' in decoded('timer-noop-clippy.log').decode()
assert decoded('timer-noop-fmt.log') == b''
for name, variant in [('timer-noop-corpus.json', 'candidate'), ('timer-noop-final-corpus.json', 'selected')]:
    corpus = receipt(name)['case']
    assert corpus['passed'] and corpus['corpus_cases'] == 776 and corpus['scheduled_exchanges'] == 2311
    assert corpus['server_sha256'] == build[variant + '_binary_sha256']
print('Corrected binary identities, census, 18 ordinary trials, 600000 exact user rows, corpus and retirement verified.')
