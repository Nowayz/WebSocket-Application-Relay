"""Audit a rejected short experiment without running traffic."""
import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'builds.json').read_text())
assert not build['selected']
rows = json.loads((ROOT / 'summary.json').read_text())
assert len(rows) == 12
for row in rows:
    name = f"drive-split-{row['kind']}-{row['trial']}-{row['variant']}.json.gz"
    x = json.loads(gzip.decompress((ROOT / 'raw' / name).read_bytes()))
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code'] == x['server_exit_code'] == 0
    assert not x['server_forced_kill']
    assert x['server_sha256'] == build[row['variant'] + '_binary_sha256']
    assert x['client_sha256'] == build['client_binary_sha256']
    assert x['args']['pairs'] == (0 if row['kind'] == 'fanout' else 64)
    assert x['args']['fanout_readers'] == 2048
    assert x['args']['seconds'] == 2
    m = x['mixed']
    n = m['healthy']['verified'] + m['fanout']['verified']
    assert row['cpu_seconds_per_million'] == m['server_cpu_seconds'] / n * 1e6
    for group in ['healthy', 'fanout']:
        assert row[group + '_per_second'] == m[group]['per_second']
        assert row[group + '_max_ms'] == m[group]['max_ms']
    done = next(e for e in x['events'] if e['event'] == 'done')
    assert done['details']['passed']
    checks = done['exact_counter_checks']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0, (name, key)
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes',
                'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total']:
        assert checks[key] == 0, (name, key)
    for key, value in done['metrics'].items():
        if any(s in key for s in ['errors_total', 'rejected_total', 'slow_consumers_total',
                                 'timeouts_total', 'aborted_writes_total', 'close_failures_total',
                                 'transport_disconnects_total', 'global_pressure_total']):
            assert value == 0, (name, key)
    print(row['kind'], row['variant'], round(row['cpu_seconds_per_million'], 4),
          round(row['fanout_max_ms'], 3))
log = gzip.decompress((ROOT / 'raw/drive-split-tests.log.gz').read_bytes()).decode()
assert '83 passed; 0 failed' in log
print('Checksums, all 12 ordinary gates, recomputed results and clean-source tests verified.')
