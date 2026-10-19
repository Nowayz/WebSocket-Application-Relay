import gzip, hashlib, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())
x=json.loads(read('admission-reuse-selected.json'))
assert x['passed'] and x['exit_code']==0
assert x['runner_sha256']==hashlib.sha256(read('admission-reuse.py')).hexdigest()
assert x['corpus_helper_sha256']==hashlib.sha256(read('input-corpus.py')).hexdigest()
build=json.loads((root/'build.json').read_text())
assert build['passed'] and not build['cargo_artifact']['fresh'] and x['server_sha256']==build['binary_sha256']
assert [c['kind'] for c in x['cases']]==['malformed-http','early-eof','timeout']*2
assert all(c['ip_refusal'] and c['slot_reused'] and c['replacement_routed'] and c['healthy_routed'] for c in x['cases'])
assert all(c['healthy_packets_during_wait']>0 for c in x['cases'] if c['kind']=='timeout')
assert x['witness_messages']==sum(c['healthy_packets_during_wait']+1 for c in x['cases'])
f=x['final_metrics']
assert f['war_ip_rejected_total']==f['war_rejected_total']==f['war_handshake_errors_total']==6
assert f['war_accepted_total']==19 and f['war_joined_total']==f['war_disconnected_total']==7
assert f['war_incoming_messages_total']==x['witness_messages']+6
assert f['war_deliveries_total']==f['war_incoming_messages_total']+7
for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes','war_routing_copied_bytes_total','war_write_errors_total','war_close_failures_total','war_transport_disconnects_total']:
    assert f[key]==0,key
assert f['war_channels']==1
corpus=json.loads(read('admission-reuse-corpus.json'))
assert corpus['script_sha256']==hashlib.sha256(read('input-corpus.py')).hexdigest()
assert corpus['case']['passed']
log=read('admission-reuse-corpus.log').decode()
assert '"corpus_cases": 776' in log and '"scheduled_exchanges": 2311' in log
manual=json.loads((root/'manual-processes.json').read_text())
assert [p['pid'] for p in manual['processes']]==[966632,1106809]
assert manual['processes'][0]['sha256'].startswith('3db9ebeb') and manual['processes'][1]['sha256'].startswith('bb2392f5')
print('Six refusals/failures/recoveries, live witness packets, exact counters/retirement, corpus, source/build identities and manual process snapshot verified.')
before=read('admission-output-before.log').decode()
assert 'FAILED (failures=2)' in before and 'partial receipt' in before
log=read('admission-output-selected-tests.log').decode()
assert 'Ran 2 tests' in log and 'OK' in log
follow=json.loads(read('admission-output-live.json'))
assert follow['passed'] and follow['exit_code']==0
assert follow['runner_sha256']==hashlib.sha256(read('admission-reuse-output-checked.py')).hexdigest()
assert len(follow['cases'])==6 and follow['final_metrics']['war_connections']==0
assert follow['final_metrics']['war_ip_rejected_total']==follow['final_metrics']['war_handshake_errors_total']==6
print('Original companion collisions, passing output/child-cleanup tests and real admission follow-up verified.')
