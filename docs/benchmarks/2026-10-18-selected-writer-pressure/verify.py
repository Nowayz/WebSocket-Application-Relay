"""Verify selected-writer unread-pressure receipt offline."""
import gzip
import hashlib
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
x = json.loads(gzip.decompress((ROOT / 'raw/receipt.json.gz').read_bytes()))
assert x['passed'] and not x['profile_diagnostic']
assert x['server_sha256'] == 'da1d1289c0ce0e5264de26ad87b756d5034576ef3cf4126b44c0f2530c1ea006'
assert x['server_exit_code'] == x['client_exit_code'] == x['pressure']['exit_code'] == 0
for name, expected in x['runner_sources_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected
pressure = x['pressure']['events'][-1]
assert pressure['verified_deliveries'] == pressure['sent_messages'] == 6246464
assert pressure['disconnect_notifications'] == pressure['unread_clients'] == 8
assert pressure['teardown_capacity_closes'] == 0
observations = pressure['unread_observations']
assert len(observations) == 8
assert sum(o['kind'] == 'close' and o['code'] == 1013 for o in observations) == 4
assert sum(o['kind'] == 'transport_error' for o in observations) == 4
end = x['events'][-1]
assert end['event'] == 'done' and end['established_server_sockets'] == 0
for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes',
            'war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
    assert end['metrics'][key] == 0, key
assert end['metrics']['war_slow_consumers_total'] == end['metrics']['war_aborted_writes_total'] == 8
assert end['metrics']['war_close_failures_total'] == 4
assert x['measurement']['round_trip']['samples'] == 3200
print('Checksums, source oracles, healthy delivery, unread outcomes and retirement verified.')
