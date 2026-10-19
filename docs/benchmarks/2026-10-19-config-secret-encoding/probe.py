"""Unix byte-environment startup check using synthetic malformed secrets only."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path.cwd()
output = ROOT / 'artifacts/config-secret-encoding-20261019-release.json'
assert not output.exists()


def free():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


cases = []
for kind in ['control', 'selected']:
    binary = ROOT / ('artifacts/candidates/worker-count-20261019-current/war-relay' if kind == 'control' else 'artifacts/candidates/config-secret-encoding-20261019-selected/war-relay')
    for name in ['WAR_AUTH_PASSWORD', 'WAR_ADMIN_TOKEN']:
        port, admin = free(), free()
        while admin == port:
            admin = free()
        environment = {key: value for key, value in os.environb.items() if not key.startswith(b'WAR_')}
        environment[name.encode()] = b'redaction-fixture-\xff-secret'
        command = [str(binary), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '1']
        child = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        record = {'kind': kind, 'variable': name, 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
        try:
            if kind == 'control':
                deadline = time.monotonic() + 2
                while True:
                    assert child.poll() is None
                    try:
                        with urllib.request.urlopen(f'http://127.0.0.1:{admin}/readyz', timeout=.2) as reply:
                            assert reply.status == 200
                        break
                    except OSError:
                        assert time.monotonic() < deadline
                        time.sleep(.005)
                with urllib.request.urlopen(f'http://127.0.0.1:{admin}/metrics', timeout=1) as reply:
                    record['unauthenticated_metrics_status'] = reply.status
                child.send_signal(signal.SIGTERM)
            stdout, stderr = child.communicate(timeout=2)
            record['exit_code'] = child.returncode
            assert child.returncode == (0 if kind == 'control' else 1)
            text = (stdout + stderr).decode(errors='replace')
            assert 'redaction-fixture' not in text
            if kind == 'selected':
                assert name in text and 'UTF-8' in text
                assert 'relay listening' not in text and 'cannot bind' not in text
            record['diagnostic'] = text
            record['passed'] = True
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate(timeout=2)
        cases.append(record)
        output.write_text(json.dumps({'passed': all(c['passed'] for c in cases), 'cases': cases}, indent=2) + '\n')
print(json.dumps({'passed': True, 'cases': len(cases)}))
