"""Recompute the observed-maxima report from retained CSVs without traffic."""
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
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected
original = ROOT.parent / '2026-10-18-simd-text-validation/raw'
receipt = json.loads(gzip.decompress((original / 'simd-text-100k.json.gz').read_bytes()))
assert receipt['passed'] and receipt['fanout_offered_load_met']
assert receipt['fanout_windows'] == {'completed': 96, 'offered': 96}
for kind, filename, hash_key, count, samples, unit in [
        ('private', 'simd-text-100k.csv', 'csv_sha256', 100000, 24, 'ms'),
        ('fanout', 'simd-text-100k-fanout.csv', 'fanout_csv_sha256', 8192, 384, 'ns')]:
    raw = gzip.decompress((original / (filename + '.gz')).read_bytes())
    digest = hashlib.sha256(raw).hexdigest()
    assert digest == receipt[hash_key]
    rows = list(csv.DictReader(io.StringIO(raw.decode())))
    assert len(rows) == len({r['id'] for r in rows}) == count
    assert all(int(r['samples']) == samples for r in rows)
    x = json.loads((ROOT / (kind + '.json')).read_text())
    assert x['statistic'] == 'max' and not x['histogram_bounds']
    case, = x['cases'].values()
    assert case['csv_sha256'] == digest and case['users'] == count
    assert case['total_packet_samples'] == count * samples
    for clock, prefix in [('send_to_receive', ''), ('intended_send_to_receive', 'schedule_')]:
        values = sorted(float(r[prefix + 'max_' + unit]) / (1 if unit == 'ms' else 1e6) for r in rows)
        metric = case['metrics'][clock]
        assert 'user_p99_ms' not in metric
        assert metric['worst_packet_ms'] == values[-1]
        expected = {'minimum': values[0], **{
            name: values[(count * percent + 99) // 100 - 1]
            for name, percent in [('median', 50), ('p95', 95), ('p99', 99), ('maximum', 100)]}}
        assert metric['user_max_ms'] == expected
        for threshold, percentage in metric['users_with_max_at_or_below_ms_percent'].items():
            assert percentage == 100 * sum(v <= float(threshold) for v in values) / count
    print(kind, count, 'users,', count * samples, 'observations verified')
tests = gzip.decompress((ROOT / 'raw/user-max-tests.log.gz').read_bytes()).decode()
assert 'Ran 6 tests' in tests and 'OK' in tests
default = json.loads(gzip.decompress((ROOT / 'raw/user-max-default-check.json.gz').read_bytes()))
assert default['passed'] and default['default_p99_values_samples_maxima_and_summary_identical']
fanout_p99 = json.loads(gzip.decompress((ROOT / 'raw/user-max-default-fanout.json.gz').read_bytes()))
assert fanout_p99['statistic'] == 'p99'
case, = fanout_p99['cases'].values()
assert 'user_p99_ms' in case['metrics']['send_to_receive']
assert 'user_max_ms' not in case['metrics']['send_to_receive']
print('Checksums, source identities, both clock distributions, thresholds, tests and default-mode receipts verified.')
