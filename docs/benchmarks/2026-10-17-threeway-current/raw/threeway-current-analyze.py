#!/usr/bin/env python3
"""Summarize every completed checked three-way trial, without dropping repeats."""
import hashlib
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parents[1]
source = ROOT / 'artifacts/threeway-current.json'
data = json.loads(source.read_text())
assert data.get('completed') and len(data['cases']) == 54
rows = []
for row in data['cases']:
    trace = ROOT / row['trace']
    assert hashlib.sha256(trace.read_bytes()).hexdigest() == row['trace_sha256']
    detail = json.loads(trace.read_text())
    assert row['passed'] == detail['passed']
    if not detail['passed']:
        assert detail['error'] == "AssertionError('inspection pacing fell behind')", detail.get('error')
    assert detail['server_sha256'] == data[row['kind'] + '_sha256']
    assert detail['client_sha256'] == data['client_sha256']
    assert detail['runner_sha256'] == data['runner_sha256']
    activity = 'fanout' if row['case'] == 'fanout' else 'healthy'
    if 'mixed' in detail:
        m = detail['mixed']
    else:
        ready, measured = detail['events']
        m = dict(measured['details'])
        m['server_cpu_seconds'] = measured['server']['cpu_seconds'] - ready['server']['cpu_seconds']
    a = m[activity]
    assert a['verified'] > 0
    assert [e['event'] for e in detail['events']] == (['ready','measured','done'] if detail['passed'] else ['ready','measured'])
    rows.append(dict(kind=row['kind'], case=row['case'], repeat=row['repetition'],
                     deliveries=a['verified'], seconds=m['elapsed_seconds'],
                     per_second=a['per_second'], cpu_us=m['server_cpu_seconds']*1e6/a['verified'],
                     cpu_seconds=m['server_cpu_seconds'], p99_upper_ms=a['p99_upper_ms'],
                     rss_mib=detail['events'][1]['server']['VmRSS']/1024,
                     query_p99_upper_ms=m['inspection']['p99_upper_ms'],
                     query_verified=m['inspection']['verified'],
                     normal_closes=detail['events'][-1]['details']['counts']['normal_closes'] if detail['passed'] else 0,
                     passed=detail['passed'], error=detail.get('error'),
                     server_exit_code=detail['server_exit_code']))
summary = {}
for case in ('private','query','fanout'):
    for kind in ('cpp','rust','modern'):
        subset = [r for r in rows if r['case']==case and r['kind']==kind]
        assert len(subset) == 6
        result = {}
        for key in ('per_second','cpu_us','p99_upper_ms','rss_mib','query_p99_upper_ms'):
            result[key] = {'median':statistics.median(r[key] for r in subset),
                           'min':min(r[key] for r in subset), 'max':max(r[key] for r in subset)}
        result['aggregate_cpu_us'] = sum(r['cpu_seconds'] for r in subset)*1e6/sum(r['deliveries'] for r in subset)
        result['aggregate_per_second'] = sum(r['deliveries'] for r in subset)/sum(r['seconds'] for r in subset)
        summary[f'{case}/{kind}'] = result
output = {'completed':True, 'passed':data['passed'], 'passed_trials':sum(r['passed'] for r in rows),
          'failed_trials':[{'kind':r['kind'],'case':r['case'],'repeat':r['repeat'],'error':r['error'],'query_verified':r['query_verified']} for r in rows if not r['passed']], 'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),
          'trials':len(rows),'verified_timed_deliveries':sum(r['deliveries'] for r in rows),
          'verified_timed_queries':sum(r['query_verified'] for r in rows),
          'normal_closes':sum(r['normal_closes'] for r in rows), 'summary':summary,'rows':rows}
(ROOT/'artifacts/threeway-current-summary.json').write_text(json.dumps(output,indent=2)+'\n')
for key,value in summary.items():
    print(key, *(f'{k}={v["median"]:.6f} [{v["min"]:.6f},{v["max"]:.6f}]' for k,v in value.items() if isinstance(v,dict)))
print('verified',output['verified_timed_deliveries'],'queries',output['verified_timed_queries'],'closes',output['normal_closes'])
