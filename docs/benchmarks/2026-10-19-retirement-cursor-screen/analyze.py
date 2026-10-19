#!/usr/bin/env python3
"""Summarize retained receipts; never launch traffic or claim selection."""
import gzip,json
from pathlib import Path
ROOT=Path(__file__).resolve().parent

def read(name):
    return json.loads(gzip.decompress((ROOT/(name+'.json.gz')).read_bytes()))

def summary():
    result={'decision':'UNSELECTED','pressure':[],'mixed':[],'private':[],'departure':[]}
    for name in ['pressure-control-first','pressure-candidate-first','pressure-1-candidate','pressure-2-control']:
        x=read(name);control='control' in name
        assert x['passed'] is (not control)
        metrics=x['pressure_metrics'][-1]
        assert (int(metrics['war_global_pressure_total'])>0) if control else int(metrics['war_global_pressure_total'])==0
        assert int(metrics['war_routing_copied_bytes_total'])==0
        if control:assert 'TimeoutError' in x['error'] and 'recipient.departure' in x['error']
        else:
            assert x['server_exit']==0
            assert [e['recipients'] for e in x['events']]==[0,100001,100000,0]
            assert int(x['after']['war_queued_bytes'])==0 and int(x['after']['war_sessions'])==100000
        result['pressure'].append({'name':name,'passed':x['passed'],'global_pressure':int(metrics['war_global_pressure_total'])})
    for name in ['mixed-1-control','mixed-2-candidate','mixed-3-candidate','mixed-4-control']:
        x=read(name);assert x['passed'] and x['correctness_passed']
        assert x['fanout_windows']=={'completed':16,'offered':16} and x['exit_codes']==[0,0,0]
        for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_native_connection_slots','war_routing_copied_bytes_total']:
            assert int(x['final_metrics'][key])==0
        m=next(e['details'] for e in x['sidecar_events'] if e['event']=='measured')
        assert m['churn']['verified']==512 and m['fanout']['verified']==524288
        assert x['events'][-1]['counts']['departure_checks']==750000
        result['mixed'].append({'name':name,'cpu_seconds':x['server_cpu_seconds'],'private_per_second':m['healthy']['per_second'],
         'worst_capacity_user_ms':x['latency']['max_ms']['max'],'fanout_max_ms':m['fanout']['max_ms'],
         'ready_rss_kib':x['before']['rss_kib']})
    for name in ['private-1-control','private-2-candidate','private-3-candidate','private-4-control']:
        x=read(name);assert x['passed'] and x['client_exit_code']==0 and x['server_exit_code']==0 and not x['server_forced_kill']
        m=x['mixed'];result['private'].append({'name':name,'verified_per_second':m['healthy']['per_second'],
          'max_ms':m['healthy']['max_ms'],'server_cpu_seconds':m['server_cpu_seconds']})
    for name in ['departure-1-control','departure-2-candidate','departure-3-candidate','departure-4-control']:
        x=read(name);assert x['passed'] and x['server_exit']==0
        assert [e['recipients'] for e in x['events']]==[0,100001,100000,0]
        assert int(x['after']['war_routing_copied_bytes_total'])==0 and int(x['after']['war_queued_bytes'])==0
        events=[]
        for e in x['events'][1:3]:
            maxima=[max(s['received_ms']-s['sent_ms'] for s in x['samples'] if s['probe']==i and e['start_ms']<=s['sent_ms']<=e['end_ms']) for i in range(8)]
            events.append({'label':e['label'],'ack_ms':e['close_ack_ms']-e['close_sent_ms'],'worst_probe_ms':max(maxima)})
        result['departure'].append({'name':name,'events':events})
    return result

if __name__=='__main__':print(json.dumps(summary(),indent=2))
