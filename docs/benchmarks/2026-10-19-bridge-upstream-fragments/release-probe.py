"""Fragmented upstream welcome/data plus eight bounded faults against a release bridge."""
import base64
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

ROOT = Path.cwd()
spec = importlib.util.spec_from_file_location('wire', ROOT / 'scripts/reference.py')
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)
FAULTS = [(b'\x83\x00', 1002, 'Protocol Error'),
          (b'\x81\x01\xff', 1007, 'Invalid UTF-8'),
          (b'\x82\x80\x00\x00\x00\x00', 1002, 'Protocol Error'),
          (b'\x82\x7f\x00\x00\x00\x00\x01\x00\x00\x01', 1009, 'Message Too Large')]


def exact(stream, size):
    data = bytearray()
    while len(data) < size:
        part = stream.recv(size - len(data))
        assert part, 'unexpected upstream EOF'
        data.extend(part)
    return bytes(data)


def fragment(payload):
    # 512 non-final empty frames plus one final continuation.
    return b'\x02\x00' + b'\x00\x00' * 511 + bytes([0x80, len(payload)]) + payload


def fake_upstream(listener, results):
    try:
        for welcomed in [False, True]:
            for fault, _, _ in FAULTS:
                stream, _ = listener.accept()
                with stream:
                    stream.settimeout(3)
                    header = bytearray()
                    while not header.endswith(b'\r\n\r\n'):
                        header.extend(exact(stream, 1))
                        assert len(header) <= 16384
                    key = next(line.split(b':', 1)[1].strip() for line in header.split(b'\r\n')
                               if line.lower().startswith(b'sec-websocket-key:'))
                    accept = base64.b64encode(hashlib.sha1(key + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
                    stream.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ' + accept + b'\r\n\r\n')
                    opcode, length = exact(stream, 2)
                    assert opcode == 0x82 and length == 0x86
                    mask = exact(stream, 4)
                    payload = exact(stream, 6)
                    assert bytes(value ^ mask[i % 4] for i, value in enumerate(payload)) == b'\x45fault'
                    stream.sendall((fragment(b'\x18\x2a') + fragment(b'\x82\x18\x2a\x42\x11\x22') if welcomed else b'') + fault)
                    try:
                        assert stream.recv(1) == b'', 'unexpected bytes on invalid upstream transport'
                    except ConnectionResetError:
                        pass
        results.put(None)
    except BaseException as error:
        results.put(repr(error))


def main():
    binary = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2])
    with output.open('x') as receipt, socket.socket() as upstream, socket.socket() as reservation, tempfile.TemporaryFile() as logs:
        upstream.bind(('127.0.0.1', 0)); upstream.listen(); upstream.settimeout(3)
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
        reservation.close()
        command = [str(binary), '--bind', f'127.0.0.1:{port}', '--upstream', f'ws://127.0.0.1:{upstream.getsockname()[1]}', '--max-connections', '1']
        results = queue.Queue()
        worker = threading.Thread(target=fake_upstream, args=(upstream, results), daemon=True)
        worker.start()
        process = subprocess.Popen(command, env={**os.environ, 'TOKIO_WORKER_THREADS': '4', 'RUST_LOG': 'info'}, stdout=logs, stderr=logs)
        record = {'passed': False, 'command': command, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'cases': []}
        try:
            deadline = time.monotonic() + 3
            while b'legacy bridge listening' not in os.pread(logs.fileno(), 65536, 0):
                assert process.poll() is None and time.monotonic() < deadline, 'bridge failed to start'
                time.sleep(.005)
            record['baseline_fds'] = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
            started = time.monotonic()
            for welcomed in [False, True]:
                for fault, expected_code, reason in FAULTS:
                    client = wire.Client(port, b'fault', reject=not welcomed)
                    try:
                        if welcomed:
                            assert client.id == 42
                            opcode, payload = client.receive()
                            assert opcode == 2 and payload == struct.pack('<Q', 42) + b'\x11\x22'
                            opcode, payload = client.receive()
                            assert opcode == 8
                        else:
                            payload = client.rejection
                        assert struct.unpack('>H', payload[:2])[0] == expected_code
                        assert payload[2:] == reason.encode()
                        assert client.socket.recv(1) == b'', 'legacy connection did not retire after Close ACK'
                        record['cases'].append({'welcomed': welcomed, 'fragmented_data_verified': welcomed, 'frames_per_fragmented_message': 513 if welcomed else 0, 'fault_hex': fault.hex(), 'close_code': expected_code, 'reason': reason, 'eof': True})
                    finally:
                        client.socket.close()
            worker.join(timeout=3)
            assert not worker.is_alive()
            error = results.get_nowait()
            assert error is None, error
            deadline = time.monotonic() + 1
            while len(list(Path(f'/proc/{process.pid}/fd').iterdir())) != record['baseline_fds']:
                assert time.monotonic() < deadline, 'bridge descriptors did not return to baseline'
                time.sleep(.005)
            record['final_fds'] = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
            record['elapsed_seconds'] = time.monotonic() - started
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
        print(json.dumps({k: record.get(k) for k in ['passed', 'elapsed_seconds', 'error', 'baseline_fds', 'final_fds', 'exit_code']}))
        assert record['passed'], record.get('error')


if __name__ == '__main__':
    main()
