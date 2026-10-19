"""Compare exact management token bytes over disposable direct HTTP/1 sockets."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

ROOT = Path.cwd()
output = ROOT / 'artifacts/admin-token-bytes-20261019-release.json'
assert not output.exists()


def free():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def http(port, path, auth=None, method='GET'):
    with socket.create_connection(('127.0.0.1', port), timeout=2) as stream:
        header = b'Authorization: ' + auth + b'\r\n' if auth is not None else b''
        stream.sendall(f'{method} {path} HTTP/1.1\r\nHost: localhost\r\n'.encode() + header + b'Content-Length: 0\r\nConnection: close\r\n\r\n')
        response = bytearray()
        while data := stream.recv(65536):
            response.extend(data)
        return int(response.split(b' ', 2)[1])


cases = []
for kind in ['control', 'selected']:
    binary = ROOT / ('artifacts/candidates/config-secret-encoding-20261019-selected/war-relay' if kind == 'control' else 'artifacts/candidates/admin-token-bytes-20261019-selected/war-relay')
    for token_kind, token in [('ascii', 'ascii-fixture'), ('utf8', 'unicode-π-世界')]:
        port, admin = free(), free()
        while port == admin:
            admin = free()
        environment = {key: value for key, value in os.environ.items() if not key.startswith('WAR_')}
        environment['WAR_ADMIN_TOKEN'] = token
        command = [str(binary), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '1']
        child = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        record = {'kind': kind, 'token_kind': token_kind, 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'statuses': []}
        try:
            deadline = time.monotonic() + 2
            while True:
                assert child.poll() is None
                try:
                    assert http(admin, '/readyz') == 200
                    break
                except OSError:
                    assert time.monotonic() < deadline
                    time.sleep(.005)
            correct = ('Bearer ' + token).encode()
            allowed = kind == 'selected' or token_kind == 'ascii'
            for path, auth, expected, method in [
                ('/metrics', None, 401, 'GET'),
                ('/v1/channels?limit=1', None, 401, 'GET'),
                ('/v1/channels?limit=1', correct + b'x', 401, 'GET'),
                ('/metrics', b'Bearer invalid-\xff', 401, 'GET'),
                ('/metrics', ('Basic ' + token).encode(), 401, 'GET'),
                ('/v1/channels?limit=1', correct, 200 if allowed else 401, 'GET'),
                ('/metrics', correct, 200 if allowed else 401, 'GET'),
                ('/v1/drain', correct, 204 if allowed else 401, 'POST'),
                ('/readyz', None, 503 if allowed else 200, 'GET'),
                ('/v1/resume', correct, 204 if allowed else 401, 'POST'),
                ('/readyz', None, 200, 'GET'),
            ]:
                status = http(admin, path, auth, method)
                assert status == expected, (kind, token_kind, path, status, expected)
                record['statuses'].append({'path': path, 'method': method, 'status': status})
            child.send_signal(signal.SIGTERM)
            stdout, stderr = child.communicate(timeout=2)
            assert child.returncode == 0
            assert token not in (stdout + stderr).decode(errors='replace')
            record.update(exit_code=child.returncode, passed=True)
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate(timeout=2)
        cases.append(record)
        output.write_text(json.dumps({'passed': len(cases) == 4 and all(case['passed'] for case in cases), 'cases': cases}, indent=2) + '\n')
print(json.dumps({'passed': True, 'cases': len(cases), 'checked_requests': sum(len(case['statuses']) for case in cases)}))
