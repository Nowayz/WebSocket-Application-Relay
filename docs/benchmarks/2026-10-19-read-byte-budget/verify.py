import gzip, hashlib, json, subprocess, tempfile
from pathlib import Path
root = Path(__file__).resolve().parent
base = gzip.decompress((root / 'base-server.rs.gz').read_bytes())
provenance = json.loads((root / 'provenance.json').read_text())
assert hashlib.sha256(base).hexdigest() == provenance['base_server_sha256']
builds = {}
for name in ['control', 'candidate', 'diagnostic', 'candidate-diagnostic']:
    build = json.loads((root / f'{name}-build.json').read_text())
    assert build['passed'] and build['cargo_artifact']['fresh'] is False
    builds[name] = build
    if name != 'control':
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            (work / 'src').mkdir()
            (work / 'src/server.rs').write_bytes(base)
            subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-i', str(root / f'{name}.patch')], cwd=work, capture_output=True, check=True)
            actual = hashlib.sha256((work / 'src/server.rs').read_bytes()).hexdigest()
            assert actual == build['source_sha256']['src/server.rs']
    else:
        assert build['source_sha256']['src/server.rs'] == provenance['base_server_sha256']
client = json.loads((root.parent / '2026-10-19-private-payload-workloads/build.json').read_text())
paths = sorted(root.glob('read-byte-budget-20261019-[0-3]-*.json'))
assert len(paths) == 4
for filename, kind, expected_max in [
    ('read-turn-20261019-diagnostic.json', 'diagnostic', 1048816),
    ('read-byte-budget-20261019-diagnostic.json', 'candidate-diagnostic', 262204),
]:
    p = json.loads((root / filename).read_text())
    profiles = [json.loads(line.split('READ_TURN_PROFILE ', 1)[1]) for line in p['server_log'].splitlines() if line.startswith('READ_TURN_PROFILE ')]
    assert len(profiles) == 4
    assert max(x['max_bytes'] for x in profiles) == expected_max
    assert max(x['max_messages'] for x in profiles) == (16 if kind == 'diagnostic' else 4)
    assert all(len(x['counts']) == len(x['nanos']) == len(x['max_nanos']) == 65 for x in profiles)
    assert p['server_sha256'] == builds[kind]['binary_sha256']
    paths.append(root / filename)
for path in paths:
    p = json.loads(path.read_text())
    assert p['passed'] and p['client_exit_code'] == p['server_exit_code'] == 0
    assert not p['server_forced_kill']
    assert p['client_sha256'] == client['binary_sha256']
    assert p['args']['private_payload_bytes'] == p['mixed']['private_payload_bytes'] == 65536
    assert p['args']['seconds'] == 2 and p['args']['pairs'] == 4 and p['args']['fanout_readers'] == 2048
    if 'diagnostic' not in path.name:
        kind = 'candidate' if 'candidate' in path.name else 'control'
        assert not p['profile_diagnostic'] and 'READ_TURN_PROFILE' not in p['server_log']
        assert p['server_sha256'] == builds[kind]['binary_sha256']
    mixed = p['mixed']
    assert mixed['fanout']['verified'] % 2048 == 0
    assert mixed['counts']['value_bytes'] == mixed['healthy']['verified'] * 65541 + mixed['fanout']['verified'] // 2048 * 1027
    for event in p['events']:
        assert event['metrics']['war_routing_copied_bytes_total'] == 0
        for key, value in event['exact_counter_checks'].items():
            assert event['metrics'][key] == value
    final = p['events'][-1]
    assert final['event'] == 'done'
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_native_connection_slots']:
        assert final['metrics'][key] == 0
print('PASS: reconstructed variants; four ordinary comparisons; two distinct turn diagnostics; exact counters and full retirement')
