import gzip, hashlib, json
from pathlib import Path
root = Path(__file__).resolve().parent
for line in (root / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ',1)
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
def read(name):
    return gzip.decompress((root / 'raw' / (name + '.gz')).read_bytes()).decode()
builds = {k:json.loads(read(k+'-build.json')) for k in ['control','one','one-confirm','two']}
a = read('control-socket.rs')
for kind,build in builds.items():
    assert build['passed'] and not build['cargo_artifact']['fresh']
    source = a
    if kind != 'control':
        n = 2 if kind == 'two' else 1
        source = a.replace('        let old_capacity = state.queue.capacity();\n','        let old_capacity = state.queue.capacity();\n' + ('        // Sparse deferred output needs one slot; bursts retain normal growth.\n' if kind == 'one-confirm' else '') + f'        if old_capacity == 0 {{\n            state.queue.reserve_exact({n});\n        }}\n')
    assert hashlib.sha256(source.encode()).hexdigest() == build['source_sha256']['src/server/socket.rs']
rows = json.loads((root/'summary.json').read_text())
assert len(rows) == 20
clients=set()
for row in rows:
    x=json.loads(read(row['receipt'])); clients.add(x['client_sha256'])
    assert x['passed'] and not x['profile_diagnostic']
    assert x['client_exit_code'] == x['server_exit_code'] == 0 and not x['server_forced_kill']
    kind='control' if row['variant']=='control' else {'one-slot':'one','one-slot-confirm':'one-confirm','two-slot':'two'}[row['experiment']]
    assert x['server_sha256'] == builds[kind]['binary_sha256']
    assert x['args']['seconds'] == 2 and x['args']['pairs'] == 64 and x['args']['fanout_readers'] == 2048
    assert x['args']['fanout_window'] == row['window']
    m=x['mixed']; n=m['healthy']['verified']+m['fanout']['verified']
    assert row['cpu_per_million'] == m['server_cpu_seconds']/n*1e6
    for label,group in [('private','healthy'),('fanout','fanout')]:
        for suffix in ['p99_upper_ms','max_ms']:
            assert row[label+'_'+suffix] == m[group][suffix]
    measured=next(e for e in x['events'] if e['event']=='measured')
    assert row['queue_capacity_bytes'] == measured['metrics']['war_output_queue_capacity_bytes']
    done=next(e for e in x['events'] if e['event']=='done')
    assert done['details']['passed']
    for key in ['war_connections','war_sessions','war_queued_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0, key
assert len(clients) == 1
for prefix in ['queue-initial','queue-initial-two']:
    assert '91 passed; 0 failed' in read(prefix+'-tests.log')
    log=read(prefix+'-clippy.log'); assert 'Finished' in log and 'error:' not in log
burst=[r for r in rows if r['experiment']=='two-slot' and r['window']==4]
assert min(r['fanout_max_ms'] for r in burst if r['variant']=='candidate') > max(r['fanout_max_ms'] for r in burst if r['variant']=='control')
print('Twenty short trials, reconstructed source/binary identities, exact queue memory and latency results, FIFO/counters/retirement and both candidate checks verified.')
