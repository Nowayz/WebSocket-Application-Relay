"""Check invalid bridge limits before an intentionally occupied bind address."""
import ctypes
import hashlib
import json
from pathlib import Path
import socket
import subprocess

ROOT = Path.cwd()
output = ROOT / 'artifacts/bridge-prebind-limits-20261019-release.json'
assert not output.exists()
cases = []
maximum = str(ctypes.c_size_t(-1).value)
for kind in ['control', 'selected']:
    binary = ROOT / ('artifacts/candidates/bridge-upstream-unavailable-20261019-selected/war-legacy-bridge' if kind == 'control' else 'artifacts/candidates/bridge-prebind-limits-20261019-selected/war-legacy-bridge')
    for flag, value, invalid in [
        ('--max-connections', '0', 'invalid bridge connection limit'),
        ('--max-connections', maximum, 'invalid bridge connection limit'),
        ('--max-connections-per-ip', '4294967296', 'max-connections-per-ip must be in 0..=4294967295'),
        ('--max-connections', '100000', None),
    ]:
        with socket.socket() as occupied:
            occupied.bind(('127.0.0.1', 0)); occupied.listen(1)
            command = [str(binary), '--bind', f'127.0.0.1:{occupied.getsockname()[1]}', flag, value]
            result = subprocess.run(command, env={'TOKIO_WORKER_THREADS': '1'}, capture_output=True, text=True, timeout=2)
        assert result.returncode == 1
        expected = invalid if kind == 'selected' and invalid else 'cannot bind legacy bridge'
        assert expected in result.stderr and 'legacy bridge listening' not in result.stdout
        cases.append({'kind': kind, 'flag': flag, 'value': value, 'invalid': bool(invalid), 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'exit_code': result.returncode, 'diagnostic': result.stderr, 'passed': True})
        output.write_text(json.dumps({'passed': len(cases) == 8, 'cases': cases}, indent=2) + '\n')
print(json.dumps({'passed': True, 'cases': len(cases)}))
