import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
def read(name):
    return gzip.decompress((root / 'raw' / (name + '.gz')).read_bytes()).decode()
builds = {k: json.loads(read(k + '-build.json')) for k in ['control', 'candidate']}
assert all(x['passed'] and not x['cargo_artifact']['fresh'] for x in builds.values())
a, b = [builds[k]['source_sha256'] for k in ['control', 'candidate']]
assert [k for k in b if a.get(k) != b[k]] == ['src/server/socket.rs']
rows = json.loads((root / 'summary.json').read_text())
assert len(rows) == 8
for kind in ['fanout', 'mixed']:
    assert [r['variant'] for r in rows if r['kind'] == kind] == ['control', 'candidate', 'candidate', 'control']
for row in rows:
    x = json.loads(read(row['receipt']))
    assert x['passed'] and not x['profile_diagnostic']
    assert x['server_exit_code'] == x['client_exit_code'] == 0 and not x['server_forced_kill']
    assert x['server_sha256'] == builds[row['variant']]['binary_sha256']
    assert x['args']['seconds'] == 2 and x['args']['fanout_readers'] == 2048
    assert x['args']['pairs'] == (64 if row['kind'] == 'mixed' else 0)
    m = x['mixed']
    assert row['cpu_per_million'] == m['server_cpu_seconds'] / (m['fanout']['verified'] + m['healthy']['verified']) * 1e6
    assert row['private_max_ms'] == m['healthy']['max_ms']
    assert row['fanout_max_ms'] == m['fanout']['max_ms']
    done = next(e for e in x['events'] if e['event'] == 'done')
    assert done['details']['passed']
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0, key
assert '90 passed; 0 failed' in read('notify-load-tests.log')
p = json.loads(read('current-fanout-profile.json'))
assert p['passed'] and p['profile_diagnostic'] and p['perf_exit_code'] in [0, -2]
assert 'Total Lost Samples: 0' in read('current-fanout-profile-report.txt')
print('Eight short ordinary trials, source/binary identities, retirement, 90 tests and separate profile verified.')
