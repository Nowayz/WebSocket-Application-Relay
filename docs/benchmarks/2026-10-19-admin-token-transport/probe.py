"""Synthetic token HTTP/1 representability and startup rejection checks."""
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

ROOT = Path.cwd()
output = ROOT / 'artifacts/admin-token-transport-20261019-release.json'
assert not output.exists()


def free():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def status(port, path, token=None):
    with socket.create_connection(('127.0.0.1', port), timeout=2) as stream:
        auth = b'Authorization: Bearer ' + token.encode() + b'\r\n' if token is not None else b''
        stream.sendall(f'GET {path} HTTP/1.1\r\nHost: localhost\r\n'.encode() + auth + b'Connection: close\r\n\r\n')
        data = bytearray()
        while part := stream.recv(65536):
            data.extend(part)
        return int(data.split(b' ', 2)[1])


fixtures = [('ascii', 'valid-fixture', True), ('unicode', 'valid-π-世界', True), ('interior-tab', 'valid\tfixture', True), ('leading-ows', ' \tvalid-fixture', True), ('unicode-nbsp', 'valid\u00a0', True), ('trailing-space', 'redaction-fixture ', False), ('trailing-tab', 'redaction-fixture\t', False)]
cases = []
for kind in ['control', 'selected']:
    binary = ROOT / ('artifacts/candidates/admin-token-bytes-20261019-selected/war-relay' if kind == 'control' else 'artifacts/candidates/admin-token-transport-20261019-selected/war-relay')
    for label, token, representable in fixtures:
        port, admin = free(), free()
        while port == admin:
            admin = free()
        environment = {key: value for key, value in os.environ.items() if not key.startswith('WAR_')}
        environment['WAR_ADMIN_TOKEN'] = token
        command = [str(binary), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '1']
        child = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        record = {'kind': kind, 'token_case': label, 'representable': representable, 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
        try:
            if kind == 'selected' and not representable:
                stdout, stderr = child.communicate(timeout=2)
                assert child.returncode == 1
                diagnostic = (stdout + stderr).decode(errors='replace')
                assert 'WAR_ADMIN_TOKEN' in diagnostic and 'HTTP' in diagnostic
                assert 'redaction-fixture' not in diagnostic and 'relay listening' not in diagnostic
                record['diagnostic'] = diagnostic
            else:
                deadline = time.monotonic() + 2
                while True:
                    assert child.poll() is None
                    try:
                        assert status(admin, '/readyz') == 200
                        break
                    except OSError:
                        assert time.monotonic() < deadline
                        time.sleep(.005)
                record['authenticated_metrics_status'] = status(admin, '/metrics', token)
                assert record['authenticated_metrics_status'] == (200 if representable else 401)
                record['unauthenticated_metrics_status'] = status(admin, '/metrics')
                assert record['unauthenticated_metrics_status'] == 401
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
        output.write_text(json.dumps({'passed': len(cases) == 14, 'cases': cases}, indent=2) + '\n')
print(json.dumps({'passed': True, 'cases': len(cases)}))
