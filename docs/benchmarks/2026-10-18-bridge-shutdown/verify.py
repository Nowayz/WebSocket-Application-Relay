"""Offline audit of disposable bridge shutdown and native witness continuity."""
import gzip,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
sha=lambda b:hashlib.sha256(b).hexdigest()
for name,h in json.loads((root/'manifest.json').read_text()).items():assert sha((root/name).read_bytes())==h,name
p=json.loads((root/'provenance.json').read_text());probe=(root/'probe.py').read_bytes()
for n in [32,128]:
 d=json.loads(gzip.decompress((root/'raw'/f'bridge-shutdown-complete-{n}.json.gz').read_bytes()))
 assert d['passed'] and d['exit_codes']==[0,0] and d['bridge_exit_code']==0 and not d.get('forced_kill_pids')
 assert d['script_sha256']==sha(probe)
 for file,h in d['helper_sha256'].items():assert h==sha(subprocess.check_output(['git','show',p['source_revision']+':'+file])),file
 assert d['server_sha256']=='3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9'
 assert d['bridge_sha256']=='bb2392f53bd8fe1bdff47f0a8e790b366269400e259839565bdeb4573f9f8bf1'
 assert d['users']==d['terminated_legacy_transports']==d['terminated_partial_http_transports']==n
 assert d['before_shutdown']['war_connections']==d['before_shutdown']['war_sessions']==n+1 and d['before_shutdown']['war_channels']==n+2
 assert d['after_shutdown']['war_connections']==d['after_shutdown']['war_sessions']==1 and d['after_shutdown']['war_channels']==2
 assert d['after_shutdown']['war_transport_disconnects_total']==d['after_shutdown']['war_disconnected_total']==n
 assert d['witness_alive_before'] and d['healthy_verified']>0 and d['post_shutdown_echo_verified'] and 0<d['bridge_exit_seconds']<3
 m=d['final'];assert m['war_channels']==1
 assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total','war_close_failures_total','war_write_errors_total','war_protocol_errors_total','war_handshake_errors_total','war_rejected_total','war_slow_consumers_total','war_global_pressure_total'])
 for k,v in d['exact_checks'].items():assert m[k]==v,k
 assert m['war_transport_disconnects_total']==n
failed=json.loads(gzip.decompress((root/'raw/bridge-shutdown-01.json.gz').read_bytes()))
assert not failed['passed'] and 'war_incoming_bytes_total' in failed['error']
print('Verified two disposable shutdown cases, both transport populations, native witness survival, exact counters/resource retirement, helper provenance and retained initial byte-count failure.')
