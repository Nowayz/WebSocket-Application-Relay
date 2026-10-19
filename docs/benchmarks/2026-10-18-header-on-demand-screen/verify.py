import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
def read(name):
    return gzip.decompress((root / 'raw' / (name + '.gz')).read_bytes()).decode()
builds = {kind: json.loads(read(kind + '-build.json')) for kind in ['control', 'candidate']}
assert all(x['passed'] and not x['cargo_artifact']['fresh'] for x in builds.values())
a, b = [builds[k]['source_sha256'] for k in ['control', 'candidate']]
assert [k for k in b if a.get(k) != b[k]] == ['src/server/socket.rs']
rows = json.loads((root / 'summary.json').read_text())
assert [r['variant'] for r in rows] == ['control', 'candidate', 'candidate', 'control']
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['passed'] and not x['profile_diagnostic']
    assert x['server_exit_code'] == x['client_exit_code'] == 0 and not x['server_forced_kill']
    assert x['server_sha256'] == builds[row['variant']]['binary_sha256']
    assert x['args']['seconds'] == 2 and x['args']['fanout_readers'] == 2048
    m = x['mixed']
    assert row['cpu_per_million'] == m['server_cpu_seconds'] / m['fanout']['verified'] * 1e6
    assert row['max_ms'] == m['fanout']['max_ms']
    assert row['deliveries_per_second'] == m['fanout']['per_second']
    done = next(e for e in x['events'] if e['event'] == 'done')
    assert done['details']['passed']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0, key
assert '90 passed; 0 failed' in read('header-on-demand-tests.log')
assert min(r['cpu_per_million'] for r in rows if r['variant'] == 'candidate') > max(r['cpu_per_million'] for r in rows if r['variant'] == 'control')
print('Four short trials, rejection, source/binary identities, resource retirement and 90 tests verified.')
