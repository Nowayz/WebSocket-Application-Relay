import hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
receipts = sorted(root.glob('private-payload-20261019-*.json'))
assert len(receipts) == 9
for path in receipts:
    p = json.loads(path.read_text())
    assert p['passed'] and p['client_exit_code'] == p['server_exit_code'] == 0
    assert not p['server_forced_kill'] and not p['profile_diagnostic']
    size = p['args']['private_payload_bytes']
    command = p['command']
    assert ('--private-payload-bytes' in command) == (size != 1024)
    mixed = p['mixed']
    if 'old-client' not in path.name:
        assert mixed['private_payload_bytes'] == size
    header = 1 if size < 24 else 2 if size <= 255 else 3 if size <= 65535 else 5
    readers = p['args']['fanout_readers']
    fanout = mixed['fanout']['verified']
    assert readers == 0 or fanout % readers == 0
    sent_fanout = fanout // readers if readers else 0
    assert mixed['counts']['value_bytes'] == mixed['healthy']['verified'] * (size + header) + sent_fanout * 1027
    for event in p['events']:
        assert event['metrics']['war_routing_copied_bytes_total'] == 0
        for key, value in event['exact_counter_checks'].items():
            assert event['metrics'][key] == value
    done = p['events'][-1]
    assert done['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']:
        assert done['metrics'][key] == 0
    slots = done['metrics']['war_native_connection_slots']
    # Empty small arenas intentionally stay warm (server::reclaim_connections).
    assert 0 <= slots <= 256 * p['args']['workers']
    if readers:
        assert slots == 0
build = json.loads((root / 'build.json').read_text())
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for path in (root / 'source' / 'examples').rglob('*.rs'):
    relative = str(path.relative_to(root / 'source'))
    assert hashlib.sha256(path.read_bytes()).hexdigest() == build['source_sha256'][relative]
print('PASS: nine checked socket workloads, encoded byte totals, zero-copy counters, retirement and frozen client sources')
