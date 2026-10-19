#!/usr/bin/env python3
import gzip,json,re,statistics
from pathlib import Path
ROOT=Path(__file__).resolve().parent

def read(name):return json.loads(gzip.decompress((ROOT/(name+'.json.gz')).read_bytes()))

def summary():
    result={'component':[],'mixed':[],'observers':[]}
    for i,kind in enumerate(['control','candidate','candidate','control'],1):
        text=gzip.decompress((ROOT/f'component-{i}-{kind}.log.gz').read_bytes()).decode()
        x=json.loads(re.search(r'WAR_WAKE_CENSUS=(\{.*\})',text)[1])
        assert x['source_drops']==1000 and len(x['elapsed_ns'])==7
        assert x['wake_calls_including_initial']==[1001 if kind=='control' else 1]*7
        assert 'test result: ok. 1 passed; 0 failed' in text
        result['component'].append({'kind':kind,'median_ns':statistics.median(x['elapsed_ns']),
                                    'wake_calls':x['wake_calls_including_initial'][0]})
    for name in ['mixed-1-control','mixed-2-candidate','mixed-3-candidate','mixed-4-control',
                 'mixed-observers-candidate','mixed-observers-control']:
        x=read(name);assert x['passed'] and x['correctness_passed']
        assert x['exit_codes']==[0,0,0] and x['fanout_windows']=={'completed':16,'offered':16}
        for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
                    'war_read_buffer_reserved_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:
            assert int(x['final_metrics'][key])==0,(name,key)
        m=next(e['details'] for e in x['sidecar_events'] if e['event']=='measured')
        assert m['fanout']['verified']==524288 and m['churn']['verified']==512
        assert x['events'][-1]['counts']['departure_checks']==750000
        row={'name':name,'server_cpu_seconds':x['server_cpu_seconds'],
             'worst_capacity_user_ms':x['latency']['max_ms']['max'],
             'private_per_second':m['healthy']['per_second'],'fanout_max_ms':m['fanout']['max_ms']}
        result['observers' if 'observers' in name else 'mixed'].append(row)
    x=read('departure-candidate');assert x['passed'] and x['server_exit']==0 and not x['server_forced_kill']
    assert [e['recipients'] for e in x['events']]==[0,100001,100000,0]
    assert int(x['after']['war_sessions'])==100000 and int(x['after']['war_queued_bytes'])==0
    assert int(x['after']['war_routing_copied_bytes_total'])==0
    result['departure']=[{'label':e['label'],'worst_probe_ms':max(s['received_ms']-s['sent_ms'] for s in x['samples'] if e['start_ms']<=s['sent_ms']<=e['end_ms'])} for e in x['events'][1:3]]
    return result

if __name__=='__main__':print(json.dumps(summary(),indent=2))
