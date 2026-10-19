"""Fixed-work empty channel-page HTTP checks; allocator diagnostics are separate."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import urllib.parse

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'scripts'))
import benchmark as runtime
import inspection
parser = argparse.ArgumentParser()
parser.add_argument('--server', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--heap-prefix', type=Path)
parser.add_argument('--requests', type=int, default=1000)
a = parser.parse_args()
assert a.requests > 0
assert not a.output.exists()
port, admin = runtime.free_port(), runtime.free_port()
while port == admin:
    admin = runtime.free_port()
command = [str(a.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '1']
environment = {key: value for key, value in os.environ.items() if not key.startswith('WAR_')}
environment.update(WAR_ADMIN_TOKEN='empty-page-fixture', RUST_LOG='warn')
if a.heap_prefix:
    assert not list(a.heap_prefix.parent.glob(a.heap_prefix.name + '*.heap'))
    environment.update(LD_PRELOAD=str(Path('/usr/lib/libtcmalloc.so').resolve()), HEAPPROFILE=str(a.heap_prefix.resolve()), HEAPPROFILESIGNAL=str(signal.SIGUSR2), HEAP_PROFILE_ALLOCATION_INTERVAL=str(100 * 1024**3), HEAP_PROFILE_INUSE_INTERVAL=str(100 * 1024**3))
path = '/v1/channels?' + urllib.parse.urlencode({'limit': 1, 'after': 'cmVfZ2xvYmw='})
headers = {'Authorization': 'Bearer empty-page-fixture'}
record = {'passed': False, 'command': command, 'server_sha256': hashlib.sha256(a.server.read_bytes()).hexdigest(), 'requests': a.requests, 'profile_diagnostic': bool(a.heap_prefix), 'captures': []}
with a.output.open('x') as receipt, tempfile.TemporaryFile() as logs:
    child = subprocess.Popen(command, env=environment, stdout=logs, stderr=logs)
    connection = http.client.HTTPConnection('127.0.0.1', admin, timeout=2)
    try:
        deadline = time.monotonic() + 2
        while True:
            assert child.poll() is None
            try:
                connection.request('GET', '/readyz')
                reply = connection.getresponse(); reply.read()
                assert reply.status == 200
                break
            except OSError:
                connection.close()
                assert time.monotonic() < deadline
                time.sleep(.005)
        def empty():
            connection.request('GET', path, headers=headers)
            reply = connection.getresponse()
            assert reply.status == 200 and reply.getheader('Content-Type') == 'application/json'
            assert reply.getheader('Content-Length') == '2' and reply.read() == b'[]'
        for _ in range(100):
            empty()
        if a.heap_prefix:
            capture = inspection.dump_heap(child.pid, a.heap_prefix.resolve()); capture['phase'] = 'baseline'; record['captures'].append(capture)
        before = runtime.snapshot(child.pid)
        started = time.monotonic()
        for _ in range(a.requests):
            empty()
        record['elapsed_seconds'] = time.monotonic() - started
        after = runtime.snapshot(child.pid)
        record['cpu_seconds'] = after['cpu_seconds'] - before['cpu_seconds']
        if a.heap_prefix:
            capture = inspection.dump_heap(child.pid, a.heap_prefix.resolve()); capture['phase'] = 'after'; record['captures'].append(capture)
        connection.request('GET', '/v1/channels?limit=1', headers=headers)
        reply = connection.getresponse(); assert reply.status == 200
        assert len(json.loads(reply.read())) == 1
        connection.request('GET', path)
        reply = connection.getresponse(); reply.read(); assert reply.status == 401
        connection.request('GET', '/metrics', headers=headers)
        reply = connection.getresponse()
        metrics = {key: int(value) for key, value in (line.split() for line in reply.read().decode().splitlines() if line.startswith('war_'))}
        keys = ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes']
        record['final_gauges'] = {key: metrics[key] for key in keys}
        assert all(value == 0 for value in record['final_gauges'].values())
        record['passed'] = True
    except BaseException as error:
        record['error'] = repr(error)
    finally:
        connection.close()
        if child.poll() is None:
            child.send_signal(signal.SIGTERM)
        record['exit_code'] = child.wait(timeout=3)
        record['passed'] = record['passed'] and child.returncode == 0
        logs.seek(0); record['log'] = logs.read().decode(errors='replace')
        json.dump(record, receipt, indent=2); receipt.write('\n')
print(json.dumps({key: record.get(key) for key in ['passed', 'error', 'elapsed_seconds', 'cpu_seconds']}))
assert record['passed'], record.get('error')
