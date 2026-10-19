#!/usr/bin/env python3
"""Verify a rejected allocation-policy experiment without launching traffic."""
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
    original = read('original-server.rs')
    candidate = read('candidate-server.rs')
    growth = b'''            // Large connection entries make doubling an expensive idle high-water
            // mark. Keep geometric growth, with less spare metadata above 4K slots.
            let capacity = self.connections.capacity();
            if capacity >= 4096 && self.connections.len() == capacity {
                self.connections.reserve_exact(capacity / 2);
            }
'''
    assert candidate.count(growth) == 1 and candidate.replace(growth, b'') == original
    control = read('control-server.rs')
    diagnostic = control[control.index(b'\n#[cfg(test)]\nmod rate_layout_diagnostic'):]
    assert b'fn report_native_connection_layout()' in diagnostic
    assert control.removesuffix(diagnostic) == original.replace(b'\n#[cfg(test)]\nmod rate_tests;\n', b'')
    clients = set()
    summary = json.loads((ROOT / 'summary.json').read_text())
    assert [row['variant'] for row in summary] == ['control', 'candidate']
    for i, row in enumerate(summary):
        variant = row['variant']
        build = json.loads((ROOT / (variant + '-build.json')).read_text())
        assert build['passed'] and build['cargo_artifact']['fresh'] is False
        source = control if variant == 'control' else candidate
        assert hashlib.sha256(source).hexdigest() == build['source_sha256']['src/server.rs']
        record = json.loads(read(row['receipt']))
        assert record['sha256']['server'] == build['binary_sha256']
        assert record['passed'] and record['correctness_passed']
        assert record['exit_codes'] == [0, 0, 0]
        assert record['fanout_windows'] == {'completed': 32, 'offered': 32}
        assert record['fanout_offered_load_met'] and not record['generator_lag']['exceeded_one_period']
        assert record['args']['users'] == 100000 and record['args']['readers'] == 8192
        assert record['args']['seconds'] == 2
        assert record['args']['pairs'] == record['args']['inspectors'] == record['args']['churn_workers'] == 0
        assert record['sha256']['runner'] == hashlib.sha256((ROOT / 'mixed-capacity.py').read_bytes()).hexdigest()
        clients.add((record['sha256']['capacity_client'], record['sha256']['sidecar']))
        assert row['rss_kib'] == record['before']['rss_kib']
        assert row['server_cpu_seconds'] == record['server_cpu_seconds']
        assert row['setup_seconds'] == record['events'][0]['details']['setup_seconds']
        stem = row['receipt'].removesuffix('.json')
        for suffix, count, samples, scale, digest, key in [
            ('.csv', 100000, 8, 1, 'csv_sha256', 'private_max_ms'),
            ('-fanout.csv', 8192, 128, 1e6, 'fanout_csv_sha256', 'fanout_max_ms')]:
            raw = read(stem + suffix)
            assert hashlib.sha256(raw).hexdigest() == record[digest]
            users = list(csv.DictReader(io.StringIO(raw.decode())))
            assert len(users) == count and len({u['id'] for u in users}) == count
            assert all(int(u['samples']) == samples for u in users)
            unit = 'ms' if scale == 1 else 'ns'
            assert max(float(u['max_' + unit]) / scale for u in users) == row[key]
        metrics = record['final_metrics']
        for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                    'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
            assert metrics[key] == 0, key
        assert metrics['war_channels'] == 1
        left = record['events'][-1]['counts']
        right = record['sidecar_events'][-1]['details']['counts']
        for metric, field in [('war_accepted_total', 'joins'), ('war_joined_total', 'joins'),
                              ('war_disconnected_total', 'normal_closes'), ('war_incoming_messages_total', 'sent_messages'),
                              ('war_incoming_bytes_total', 'sent_bytes'), ('war_deliveries_total', 'deliveries'),
                              ('war_delivered_bytes_total', 'delivered_bytes'), ('war_routing_reused_bytes_total', 'value_bytes')]:
            assert metrics[metric] == left[field] + right[field]
    assert len(clients) == 1
    assert b'96 passed; 0 failed' in (ROOT / 'candidate-tests.log').read_bytes()
    assert summary[0]['rss_kib'] - summary[1]['rss_kib'] == 2036
    print('PASS: isolated candidate, source/build identities, 96 tests, both complete population receipts and all user rows')


if __name__ == '__main__':
    main()
