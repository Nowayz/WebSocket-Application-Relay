import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
rows=json.loads(gzip.decompress((p/'receipts.json.gz').read_bytes()))
assert [r['variant'] for r in rows]==['control','candidate','candidate','control']
for r in rows:
 assert r['passed'] and r['server_exit_code']==r['client_exit_code']==0
 assert r['client_output'].strip()=='verified 100000 boolean acknowledgements'
 assert abs(r['cpu_seconds']-(r['after']['cpu_seconds']-r['before']['cpu_seconds']))<1e-9
 m=r['metrics'];assert m['war_incoming_messages_total']==100000 and m['war_deliveries_total']==100008
 assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==8
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:assert m[k]==0
 for k,v in m.items():
  if any(w in k for w in ['errors_total','rejected_total','slow_consumers_total','timeouts_total','close_failures_total','transport_disconnects_total','global_pressure_total']):assert v==0
print('Verified four fixed-work receipts, exact counts, cleanup and CPU deltas.')
