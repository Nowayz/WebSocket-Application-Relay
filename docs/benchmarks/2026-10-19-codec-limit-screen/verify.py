#!/usr/bin/env python3
"""Audit an unselected codec change and retained public-behavior tests offline."""
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
    builds = {v: json.loads((ROOT / (v + '-build.json')).read_text()) for v in ['control', 'candidate']}
    assert builds['candidate']['passed'] and builds['candidate']['cargo_artifact']['fresh'] is False
    for variant, prefix in [('control', 'before'), ('candidate', 'candidate')]:
        for label, source in [('context', 'vendor/tungstenite/src/protocol/mod.rs'),
                              ('frame', 'vendor/tungstenite/src/protocol/frame/mod.rs')]:
            assert hashlib.sha256(read(prefix + '-' + label + '.rs')).hexdigest() == builds[variant]['source_sha256'][source]
    differences = {name for name in builds['control']['source_sha256'].keys() | builds['candidate']['source_sha256'].keys()
                   if builds['control']['source_sha256'].get(name) != builds['candidate']['source_sha256'].get(name)}
    assert differences == {'vendor/tungstenite/src/protocol/mod.rs', 'vendor/tungstenite/src/protocol/frame/mod.rs',
                           'src/server.rs', 'src/server/codec_tests.rs'}
    original, selected = read('original-server.rs'), read('selected-server.rs')
    assert selected.replace(b'#[cfg(test)]\nmod codec_tests;\n', b'') == original
    assert read('codec_tests.rs').count(b'#[test]') == 3
    assert b'max_out_buffer_len: usize,' in read('before-frame.rs')
    assert b'    max_out_buffer_len: usize,\n' not in read('candidate-frame.rs').split(b'impl FrameCodec')[0]
    assert b'connection=520 stage=320 upgrade=288 websocket=320' in (ROOT / 'before-layout.log').read_bytes()
    assert b'connection=504 stage=304 websocket=304 upgrade=288' in (ROOT / 'codec-limits-layout.log').read_bytes()
    clients = set()
    summary = json.loads((ROOT / 'summary.json').read_text())
    assert [row['variant'] for row in summary] == ['control', 'candidate', 'candidate', 'control']
    for row in summary:
        r = json.loads(read(row['receipt']))
        assert r['passed'] and r['correctness_passed'] and r['exit_codes'] == [0, 0, 0]
        assert r['sha256']['server'] == builds[row['variant']]['binary_sha256']
        assert r['fanout_windows'] == {'completed': 32, 'offered': 32} and r['fanout_offered_load_met']
        assert r['args']['users'] == 100000 and r['args']['readers'] == 8192 and r['args']['seconds'] == 2
        assert r['args']['inspectors'] == r['args']['pairs'] == r['args']['churn_workers'] == 0
        assert r['ready_metrics']['war_native_connection_slots'] == 131072
        assert row['rss_kib'] == r['before']['rss_kib'] and row['server_cpu_seconds'] == r['server_cpu_seconds']
        clients.add((r['sha256']['capacity_client'], r['sha256']['sidecar']))
        for suffix, count, samples, scale, key, summary_key in [
            ('.csv', 100000, 8, 1, 'csv_sha256', 'private_max_ms'),
            ('-fanout.csv', 8192, 128, 1e6, 'fanout_csv_sha256', 'fanout_max_ms')]:
            data = read(row['receipt'].removesuffix('.json') + suffix)
            assert hashlib.sha256(data).hexdigest() == r[key]
            users = list(csv.DictReader(io.StringIO(data.decode())))
            assert len(users) == count and len({u['id'] for u in users}) == count
            assert all(int(u['samples']) == samples for u in users)
            values = sorted(float(u['max_ms' if scale == 1 else 'max_ns']) / scale for u in users)
            assert values[-1] == row[summary_key]
            if scale == 1:
                assert values[(count - 1) * 99 // 100] == row['private_p99_user_max_ms']
        final = r['final_metrics']
        for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                    'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes',
                    'war_routing_copied_bytes_total', 'war_native_connection_slots']:
            assert final[key] == 0, key
        left, right = r['events'][-1]['counts'], r['sidecar_events'][-1]['details']['counts']
        for metric, field in [('war_accepted_total', 'joins'), ('war_joined_total', 'joins'),
                              ('war_disconnected_total', 'normal_closes'), ('war_incoming_messages_total', 'sent_messages'),
                              ('war_incoming_bytes_total', 'sent_bytes'), ('war_deliveries_total', 'deliveries'),
                              ('war_delivered_bytes_total', 'delivered_bytes'), ('war_routing_reused_bytes_total', 'value_bytes')]:
            assert final[metric] == left[field] + right[field]
    assert len(clients) == 1
    assert b'100 passed; 0 failed' in (ROOT / 'codec-limits-expanded-tests.log').read_bytes()
    assert b'100 passed; 0 failed' in (ROOT / 'codec-limits-selected-tests.log').read_bytes()
    assert b'3 passed; 0 failed' in (ROOT / 'codec-limits-control-tests.log').read_bytes()
    assert b'Finished' in (ROOT / 'codec-limits-selected-clippy.log').read_bytes()
    control = [r['rss_kib'] for r in summary if r['variant'] == 'control']
    candidate = [r['rss_kib'] for r in summary if r['variant'] == 'candidate']
    assert min(candidate) < min(control) < max(candidate) < max(control)
    print('PASS: source identities, layout probes, four complete population cases and test-only selected change')


if __name__ == '__main__':
    main()
