"""Short actual native capacity rejection through a one-slot legacy bridge."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.request

root = Path.cwd()
parser = argparse.ArgumentParser()
parser.add_argument('--bridge', type=Path, required=True)
parser.add_argument('--original', action='store_true')
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
assert not os.path.lexists(args.output)
def free():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0)); return s.getsockname()[1]
ports = set()
while len(ports) < 3: ports.add(free())
native, admin, legacy = ports
server = root / 'artifacts/candidates/connection-slots-selected/war-relay'
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
commands = [[str(server), '--bind', f'127.0.0.1:{native}', '--admin-bind', f'127.0.0.1:{admin}',
             '--workers', '2', '--max-channel-members', '1', '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0'],
            [str(args.bridge.resolve()), '--bind', f'127.0.0.1:{legacy}', '--upstream', f'ws://127.0.0.1:{native}',
             '--max-connections', '1', '--max-connections-per-ip', '1']]
env = {k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
env.update(RUST_LOG='info', WAR_ADMIN_TOKEN='bridge-welcome-fixture', WAR_TEST_URL=f'ws://127.0.0.1:{native}',
           WAR_TEST_BRIDGE_URL=f'ws://127.0.0.1:{legacy}', WAR_TEST_ADMIN=f'http://127.0.0.1:{admin}',
           WAR_EXPECT_ORIGINAL=str(int(args.original)))
record = {'passed':False, 'original':args.original, 'commands':commands,
          'server_sha256':sha(server), 'bridge_sha256':sha(args.bridge),
          'client_sha256':sha('clients/js/relay.mjs'), 'test_sha256':sha('artifacts/bridge-welcome-native.test.mjs')}
processes, logs = [], []
def metrics():
    req=urllib.request.Request(f'http://127.0.0.1:{admin}/metrics',headers={'Authorization':'Bearer bridge-welcome-fixture'})
    with urllib.request.urlopen(req,timeout=2) as response:
        return {k:int(v) for k,v in (s.split() for s in response.read().decode().splitlines() if s.startswith('war_'))}
try:
    for name, cmd in zip(['native', 'bridge'], commands):
        log = open(str(args.output.with_suffix('')) + '-' + name + '.log', 'x'); logs.append(log)
        processes.append(subprocess.Popen(cmd, env=env, stdout=log, stderr=log))
    deadline = time.monotonic() + 5
    while True:
        assert all(p.poll() is None for p in processes)
        try:
            metrics()
            if 'legacy bridge listening' in Path(logs[1].name).read_text(): break
        except OSError: pass
        assert time.monotonic() < deadline
        time.sleep(.01)
    record['before_fds'] = [len(list(Path(f'/proc/{p.pid}/fd').iterdir())) for p in processes]
    cmd = ['node', 'artifacts/bridge-welcome-native.test.mjs']
    result = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=15)
    record.update(test_command=cmd, test_exit_code=result.returncode, test_output=result.stdout+result.stderr)
    assert result.returncode == 0, record['test_output']
    deadline = time.monotonic() + 3
    while True:
        m = metrics()
        fds = [len(list(Path(f'/proc/{p.pid}/fd').iterdir())) for p in processes]
        if m['war_connections'] == m['war_sessions'] == 0 and fds == record['before_fds']: break
        assert time.monotonic() < deadline, (m, fds)
        time.sleep(.01)
    record.update(final_metrics=m, after_fds=fds)
    for key in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:
        assert m[key] == 0
    assert m['war_accepted_total'] == 4 and m['war_joined_total'] == m['war_disconnected_total'] == 2
    assert m['war_join_rejected_total'] == m['war_rejected_total'] == 2
    record['passed'] = True
finally:
    for p in reversed(processes):
        if p.poll() is None: p.terminate()
        p.wait(timeout=10)
    record['exit_codes'] = [p.returncode for p in processes]
    for log in logs: log.close()
    args.output.write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'passed':record['passed'],'output':str(args.output)}))
