"""Audit retained short trials and finite capacity acceptance without traffic."""
import csv
import gzip
import hashlib
import io
import json
import statistics
from pathlib import Path

ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())


def receipt(name):
    return json.loads(decoded(name + '.json'))


build = json.loads((ROOT / 'builds.json').read_text())
summary = json.loads((ROOT / 'summary.json').read_text())
for row in summary['short']:
    name = f"simd-text-{row['kind']}-{row['trial']}-{row['variant']}"
    x = receipt(name)
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code'] == x['server_exit_code'] == 0
    assert not x['server_forced_kill']
    assert x['server_sha256'] == build[row['variant'] + '_binary_sha256']
    assert x['client_sha256'] == build['client_binary_sha256']
    m = x['mixed']
    n = m['healthy']['verified'] + m['fanout']['verified']
    assert row['cpu_seconds_per_million'] == m['server_cpu_seconds'] / n * 1e6
    for group in ['healthy', 'fanout']:
        assert row[group + '_per_second'] == m[group]['per_second']
        assert row[group + '_max_ms'] == m[group]['max_ms']
    done = next(e for e in x['events'] if e['event'] == 'done')
    assert done['details']['passed']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0, (name, key)

for variant, name in [('control', 'simd-text-100k-control'),
                      ('candidate', 'simd-text-100k')]:
    x = receipt(name)
    assert x['passed'] and x['exit_codes'] == [0, 0, 0]
    assert not x['args']['profile']
    assert x['sha256']['server'] == build[variant + '_binary_sha256']
    assert x['fanout_windows'] == {'completed': 96, 'offered': 96}
    assert x['fanout_offered_load_met'] and not x['generator_lag']['exceeded_one_period']
    for suffix, field in [('.csv', 'csv_sha256'), ('-fanout.csv', 'fanout_csv_sha256'),
                          ('.windows.json', 'latency_windows_sha256')]:
        assert hashlib.sha256(decoded(name + suffix)).hexdigest() == x[field]
    for suffix, count, samples, field, scale, reported in [
            ('.csv', 100000, 24, 'max_ms', 1, x['latency']['max_ms']),
            ('-fanout.csv', 8192, 384, 'max_ns', 1e6, x['fanout_exact_ms']['max_ms'])]:
        rows = list(csv.DictReader(io.StringIO(decoded(name + suffix).decode())))
        assert len(rows) == len({r['id'] for r in rows}) == count
        assert all(int(r['samples']) == samples for r in rows)
        values = sorted(float(r[field]) / scale for r in rows)
        for key, p in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]:
            assert values[(count - 1) * p // 100] == reported[key]
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                'war_routing_copied_bytes_total']:
        assert x['final_metrics'][key] == 0, (name, key)
    for key, value in x['final_metrics'].items():
        if any(s in key for s in ['errors_total', 'rejected_total', 'slow_consumers_total',
                                 'timeouts_total', 'aborted_writes_total', 'close_failures_total',
                                 'transport_disconnects_total', 'global_pressure_total']):
            assert value == 0, (name, key)
    row = next(r for r in summary['population'] if r['variant'] == variant)
    assert row['cpu_seconds'] == x['server_cpu_seconds']
    assert row['private_maxima_ms'] == x['latency']['max_ms']
    assert row['fanout_maxima_ms'] == x['fanout_exact_ms']['max_ms']

print('Whole-envelope parse process medians (ns):')
medians = {}
for i, variant in enumerate(['control', 'candidate', 'candidate', 'control']):
    rows = [json.loads(line) for line in decoded(f'simd-text-cost-{i}-{variant}.jsonl').splitlines()]
    assert len(rows) == 20
    for row in rows:
        assert len(row['ns']) == 7
        medians.setdefault((row['length'], row['unicode']), {}).setdefault(variant, []).append(
            statistics.median(row['ns']))
for key, variants in medians.items():
    print(key, {v: [round(n, 2) for n in ns] for v, ns in variants.items()})

corpus = receipt('simd-text-corpus')['case']
assert corpus['passed'] and corpus['corpus_cases'] == 776 and corpus['scheduled_exchanges'] == 2311
assert corpus['server_sha256'] == build['candidate_binary_sha256']
compat = receipt('simd-text-js-check')
assert compat['passed'] and compat['test_exit_code'] == 0 and compat['exit_codes'] == [0, 0]
assert compat['server_sha256'] == build['candidate_binary_sha256']
assert compat['bridge_sha256'] == build['bridge_binary_sha256']
profile = receipt('simd-text-profile')
assert profile['passed'] and profile['profile_diagnostic']
assert profile['server_sha256'] == build['candidate_binary_sha256']
assert profile['client_sha256'] == build['client_binary_sha256']
assert 'Total Lost Samples: 0' in decoded('simd-text-profile.symbols.txt').decode()
print('Checksums, 12 ordinary trials, two full-population runs, borrowed-text corpus and compatibility verified.')
