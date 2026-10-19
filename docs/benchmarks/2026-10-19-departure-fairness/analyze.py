#!/usr/bin/env python3
"""Offline validation and summaries; does not launch traffic or overwrite evidence."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def summarize(path):
    data = json.loads(path.read_text())
    assert data['passed'] and data['server_exit'] == 0
    expected = [0, data['audience'] + 1, data['audience'], 0]
    assert [e['recipients'] for e in data['events']] == expected
    assert int(data['after']['war_sessions']) == data['audience']
    assert int(data['after']['war_queued_bytes']) == 0
    assert int(data['after']['war_routing_copied_bytes_total']) == 0
    fixture = ROOT / ('fixture-v2.py' if path.name != '100k-repeat.json' else 'fixture.py')
    assert hashlib.sha256(fixture.read_bytes()).hexdigest() == data['fixture_sha256']
    events = []
    for event in data['events']:
        maxima = []
        counts = []
        for index in range(8):
            samples = [s for s in data['samples'] if s['probe'] == index
                       and event['start_ms'] <= s['sent_ms'] <= event['end_ms']]
            assert samples
            assert all(s['sent_ms'] <= s['received_ms'] for s in samples)
            counts.append(len(samples))
            maxima.append(max(s['received_ms'] - s['sent_ms'] for s in samples))
        events.append({'label': event['label'], 'recipients': event['recipients'],
                       'close_ack_ms': event['close_ack_ms'] - event['close_sent_ms'],
                       'per_probe_max_rtt_ms': maxima, 'per_probe_samples': counts})
    return {'audience': data['audience'], 'setup_seconds': data['setup_seconds'],
            'events': events}


if __name__ == '__main__':
    for name in ['small.json', '100k-first.json', '100k-repeat.json']:
        print(name, json.dumps(summarize(ROOT / name)))
