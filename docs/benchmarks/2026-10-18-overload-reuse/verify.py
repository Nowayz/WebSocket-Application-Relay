"""Audit finite overload/admission reuse receipts without starting processes."""
import gzip
import hashlib
import json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name

def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())

meta = json.loads((root/'builds.json').read_text())
servers = set()
for name in meta['receipts']:
    x = json.loads(read(name))
    assert x['passed'] and x['exit_code'] == 0 and len(x['cases']) == 3
    assert x['baseline_fds'] == x['final_fds'] == 26
    servers.add(x['server_sha256'])
    for source, expected in x['source_sha256'].items():
        assert hashlib.sha256(read(source)).hexdigest() == expected, source
    assert x['pressure_messages'] == 6 and x['checked_control_messages'] == 15
    command = x['command']
    for option, value in [('--workers','4'), ('--max-connections','3'), ('--max-connections-per-ip','1'), ('--queue-bytes','65546'), ('--queue-messages','2'), ('--total-queue-bytes','262144')]:
        assert command[command.index(option)+1] == value
    identities = []
    for wave, case in enumerate(x['cases']):
        assert case['wave'] == wave and case['pressure_messages'] == 2
        assert case['ip_quota_refused'] and case['global_quota_refused']
        assert case['same_ip_rejoined_and_routed'] and case['healthy_packets_during_pressure'] == 2
        assert case['terminal'] == 'transport-ended' and case['buffered_frames_verified'] == 0
        assert [s['war_queued_bytes'] for s in case['pressure_samples']] == [32781, 0]
        assert [s['war_slow_consumers_total'] for s in case['pressure_samples']] == [wave, wave+1]
        assert case['overloaded']['war_sessions'] == 2
        assert case['released']['war_connections'] == case['released']['war_sessions'] == 2
        assert case['released']['war_queued_bytes'] == 0
        identities.extend([case['slow_id'], case['replacement_id']])
    assert len(set(identities)) == 6
    final = x['final_metrics']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_protocol_errors_total','war_handshake_errors_total','war_write_timeouts_total','war_rate_limited_total','war_global_pressure_total','war_routing_copied_bytes_total']:
        assert final[key] == 0, key
    assert final['war_channels'] == 1
    assert final['war_accepted_total'] == 14 and final['war_rejected_total'] == 6
    assert final['war_ip_rejected_total'] == final['war_slow_consumers_total'] == final['war_aborted_writes_total'] == 3
    assert final['war_joined_total'] == final['war_disconnected_total'] == 8
    assert final['war_incoming_messages_total'] == x['pressure_messages'] + x['checked_control_messages'] == 21
    assert final['war_incoming_bytes_total'] == x['pressure_messages']*32781 + x['checked_control_messages']*12 == 196866
assert len(servers) == 1
log = read('overload-reuse-final-output-tests.log').decode()
assert '4 tests' in log and 'OK' in log
print('Verified two fresh probes, six overload/reuse waves, quota enforcement, healthy control/input counters, pre-shutdown descriptor/resource return and evidence/cleanup tests.')
