"""Two real upstream refusals and one ten-second stalled-handshake deadline."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import queue
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

spec = importlib.util.spec_from_file_location('wire', Path.cwd() / 'scripts/reference.py')
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)


class DeadlineClient(wire.Client):
    def exact(self, size):
        self.socket.settimeout(12)
        return super().exact(size)


def main():
    binary, output = Path(sys.argv[1]).resolve(), Path(sys.argv[2])
    with output.open('x') as receipt, socket.socket() as upstream, socket.socket() as reservation, tempfile.TemporaryFile() as logs:
        upstream.bind(('127.0.0.1', 0))  # Reserve the endpoint without listening.
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
        reservation.close()
        command = [str(binary), '--bind', f'127.0.0.1:{port}', '--upstream', f'ws://127.0.0.1:{upstream.getsockname()[1]}', '--max-connections', '1']
        process = subprocess.Popen(command, env={**os.environ, 'TOKIO_WORKER_THREADS': '4', 'RUST_LOG': 'info'}, stdout=logs, stderr=logs)
        record = {'passed': False, 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'cases': []}
        result = queue.Queue()
        try:
            deadline = time.monotonic() + 3
            while b'legacy bridge listening' not in os.pread(logs.fileno(), 65536, 0):
                assert process.poll() is None and time.monotonic() < deadline
                time.sleep(.005)
            record['baseline_fds'] = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
            for name in ['refused-0', 'refused-1', 'stalled-upgrade']:
                if name == 'stalled-upgrade':
                    upstream.listen(1)
                    upstream.settimeout(3)
                    def stall():
                        try:
                            stream, _ = upstream.accept()
                            with stream:
                                stream.settimeout(12)
                                header = bytearray()
                                while not header.endswith(b'\r\n\r\n'):
                                    part = stream.recv(1)
                                    assert part
                                    header.extend(part)
                                    assert len(header) <= 16384
                                assert stream.recv(1) == b'', 'timed-out upstream did not retire'
                            result.put(None)
                        except BaseException as error:
                            result.put(repr(error))
                    worker = threading.Thread(target=stall, daemon=True)
                    worker.start()
                started = time.monotonic()
                client = DeadlineClient(port, b'recover', reject=True)
                try:
                    payload = client.rejection
                    assert struct.unpack('>H', payload[:2])[0] == 1013
                    assert payload[2:] == b'Upstream Unavailable'
                    assert client.socket.recv(1) == b''
                finally:
                    client.socket.close()
                elapsed = time.monotonic() - started
                if name == 'stalled-upgrade':
                    assert 9.5 <= elapsed <= 12, elapsed
                    worker.join(timeout=1)
                    assert not worker.is_alive() and result.get_nowait() is None
                record['cases'].append({'case': name, 'close_code': 1013, 'reason': 'Upstream Unavailable', 'eof': True, 'elapsed_seconds': elapsed})
            deadline = time.monotonic() + 1
            while len(list(Path(f'/proc/{process.pid}/fd').iterdir())) != record['baseline_fds']:
                assert time.monotonic() < deadline
                time.sleep(.005)
            record['final_fds'] = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
            record['passed'] = True
        except BaseException as error:
            record['error'] = repr(error)
        finally:
            process.send_signal(signal.SIGTERM)
            record['exit_code'] = process.wait(timeout=3)
            record['passed'] = record['passed'] and record['exit_code'] == 0
            logs.seek(0)
            record['log'] = logs.read().decode(errors='replace')
            json.dump(record, receipt, indent=2); receipt.write('\n')
        print(json.dumps({k: record.get(k) for k in ['passed', 'cases', 'error', 'baseline_fds', 'final_fds', 'exit_code']}))
        assert record['passed'], record.get('error')


if __name__ == '__main__':
    main()
