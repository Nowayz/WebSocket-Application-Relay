#!/usr/bin/env python3
"""Audit portable receipts and complete user distributions without starting traffic."""
import csv
import gzip
import hashlib
import io
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def read(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes())


def main():
    for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    summaries = json.loads((ROOT / 'summary.json').read_text())
    builds = {kind: json.loads((ROOT / (kind + '-build.json')).read_text()) for kind in ['rust', 'cpp']}
    for i, kind in enumerate(['rust', 'cpp', 'cpp', 'rust']):
        stem = f'short-current-comparison-{i}-{kind}'
        record = json.loads(read(stem + '.json'))
        assert record['passed'] and record['correctness_passed']
        assert record['exit_codes'] == [0, 0, 0]
        assert record['fanout_windows'] == {'completed': 32, 'offered': 32}
        assert record['fanout_offered_load_met']
        assert not record['generator_lag']['exceeded_one_period']
        assert record['sha256']['server'] == builds[kind]['binary_sha256']
        for key in ['war_connections', 'war_queued_bytes']:
            assert record['final_metrics'][key] == 0
        if kind == 'rust':
            for key in ['war_sessions', 'war_read_buffer_reserved_bytes',
                        'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
                assert record['final_metrics'][key] == 0
        summary = summaries[i]
        assert summary['rss_kib'] == record['before']['rss_kib']
        assert summary['cpu_seconds'] == record['server_cpu_seconds']
        for mode, suffix, size, samples, scale, digest_key, field in [
            ('private', '.csv', 100000, 8, 1, 'csv_sha256', 'latency'),
            ('fanout', '-fanout.csv', 8192, 128, 1e6, 'fanout_csv_sha256', 'fanout_exact_ms')]:
            raw = read(stem + suffix)
            assert hashlib.sha256(raw).hexdigest() == record[digest_key]
            rows = list(csv.DictReader(io.StringIO(raw.decode())))
            assert len(rows) == size and len({row['id'] for row in rows}) == size
            assert all(int(row['samples']) == samples for row in rows)
            unit = 'ms' if scale == 1 else 'ns'
            plots = json.loads((ROOT / (mode + '.json')).read_text())
            label = f"{'Rust' if kind == 'rust' else 'C++'} {1 if i in [0, 1] else 2}"
            plotted = plots['cases'][label]
            assert plotted['csv_sha256'] == record[digest_key]
            assert plotted['users'] == size and plotted['total_packet_samples'] == size * samples
            for prefix, metric in [('', 'send_to_receive'), ('schedule_', 'intended_send_to_receive')]:
                values = sorted(float(row[prefix + 'max_' + unit]) / scale for row in rows)
                ranks = {name: values[(size - 1) * q // 100]
                         for name, q in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]}
                assert ranks == record[field][prefix + 'max_ms']
                plotted_metric = plotted['metrics'][metric]
                for name, q in [('median', 50), ('p95', 95), ('p99', 99)]:
                    assert plotted_metric['user_max_ms'][name] == values[(size * q + 99) // 100 - 1]
                for limit, fraction in plotted_metric['users_with_max_at_or_below_ms_percent'].items():
                    assert fraction == 100 * sum(value <= float(limit) for value in values) / size
    print('PASS: four matched receipts, 400000 private-user and 32768 fanout-reader rows, hashes and plot thresholds')


if __name__ == '__main__':
    main()
