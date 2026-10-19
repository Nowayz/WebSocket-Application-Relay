#!/usr/bin/env python3
"""Recompute the focused comparison from retained ordinary-server receipts."""
import gzip
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent


def read(name):
    return json.loads(gzip.decompress((ROOT / (name+'.json.gz')).read_bytes()))


def departure(name):
    x = read(name)
    assert x['passed'] and x['server_exit'] == 0 and x['audience'] == 100000
    assert [e['recipients'] for e in x['events']] == [0, 100001, 100000, 0]
    assert int(x['after']['war_sessions']) == 100000
    assert int(x['after']['war_queued_bytes']) == 0
    assert int(x['after']['war_routing_copied_bytes_total']) == 0
    events = []
    for event in x['events']:
        per_probe = []
        for index in range(8):
            samples = [s for s in x['samples'] if s['probe'] == index and
                       event['start_ms'] <= s['sent_ms'] <= event['end_ms']]
            assert samples
            per_probe.append(max(s['received_ms']-s['sent_ms'] for s in samples))
        events.append({'label': event['label'], 'close_ack_ms': event['close_ack_ms']-event['close_sent_ms'],
                       'per_probe_max_ms': per_probe, 'max_ms': max(per_probe)})
    return {'name': name, 'events': events}


def mixed(name):
    x = read(name)
    assert x['passed'] and x['correctness_passed'] and x['fanout_offered_load_met']
    assert x['fanout_windows'] == {'completed': 16, 'offered': 16}
    assert x['exit_codes'] == [0, 0, 0]
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
                'war_read_buffer_reserved_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:
        assert int(x['final_metrics'][key]) == 0, (name, key)
    m = next(e['details'] for e in x['sidecar_events'] if e['event'] == 'measured')
    assert m['churn']['verified'] == 512
    assert m['fanout']['verified'] == 16*4*8192
    assert x['events'][-1]['counts']['departure_checks'] == 750000
    return {'name': name, 'server_cpu_seconds': x['server_cpu_seconds'],
            'worst_capacity_user_ms': x['latency']['max_ms']['max'],
            'private_verified_per_second': m['healthy']['per_second'],
            'private_max_ms': m['healthy']['max_ms'], 'fanout_max_ms': m['fanout']['max_ms']}


def summary():
    return {'departure': [departure(n) for n in ['departure-control-first','departure-selected-first',
                                                'departure-selected-repeat','departure-control-last']],
            'mixed': [mixed(n) for n in ['mixed-control-first','mixed-selected-first',
                                       'mixed-selected-repeat','mixed-control-last']]}


if __name__ == '__main__':
    print(json.dumps(summary(), indent=2))
