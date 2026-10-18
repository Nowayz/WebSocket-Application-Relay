#!/usr/bin/env python3
"""Recheck stage clock conservation, complete cohorts and correlated worst probes."""
import csv, hashlib, json
from pathlib import Path

def verify(prefix):
    case = json.loads(prefix.with_suffix('.json').read_text())
    assert case['passed'] and all(code == 0 for code in case['exit_codes'])
    rows = list(csv.DictReader(prefix.with_suffix('.csv').open()))
    assert len(rows) == 100000 and len({r['id'] for r in rows}) == 100000
    assert all(int(r['samples']) == 32 for r in rows)
    assert hashlib.sha256(prefix.with_suffix('.csv').read_bytes()).hexdigest() == case['csv_sha256']
    fanout = prefix.with_name(prefix.name + '-fanout.csv')
    assert hashlib.sha256(fanout.read_bytes()).hexdigest() == case['fanout_csv_sha256']
    audience = list(csv.DictReader(fanout.open()))
    assert len(audience) == 8192 and len({r['id'] for r in audience}) == 8192
    details = next(e['details']['fanout'] for e in case['sidecar_events'] if e['event'] == 'measured')
    assert sum(int(r['samples']) for r in audience) == details['verified']
    timeline = json.loads(prefix.with_suffix('.windows.json').read_text())
    assert len(timeline['windows']) == 8
    identities = {int(r['id']) for r in rows}
    for second,w in enumerate(timeline['windows']):
        assert w['intended_second'] == second
        if not case['args']['latency_stages']:
            assert not w['client_stages'] and all(p is None for p in w['worst_stage_probes'].values())
            continue
        assert set(w['client_stages']) == set(timeline['stage_order'])
        for stage in w['client_stages'].values():
            assert stage['samples'] == 400000
            assert 0 <= stage['p50_ms'] <= stage['p95_ms'] <= stage['p99_ms'] <= stage['max_ms']
        for label,probe in w['worst_stage_probes'].items():
            stages = probe['stages_ns']
            assert sum(stages) == probe['intended_ns']
            assert probe['id'] in identities and (probe['sequence']-1)//4 == second
            observed = {'intended_to_receive':probe['intended_ns'], 'pre_send_delay':sum(stages[:3]),'after_send':stages[4]}[label]
            expected = (w['client_stages']['after_send'] if label=='after_send' else w[label])['max_ms']
            assert abs(observed / 1e6 - expected) < 1e-9
    return {'prefix':str(prefix),'passed':True,'private_probes':3200000,'instrumented':case['args']['latency_stages']}

if __name__ == '__main__':
    import sys
    results = [verify(Path(p)) for p in sys.argv[1:]]
    print(json.dumps({'passed':True,'runs':results},indent=2))
