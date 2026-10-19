#!/usr/bin/env python3
"""Finite cross-room departure test with one deliberately backpressured sender.

A full local output budget forces synchronous retirement fallback. Healthy peers
must still receive every expected notice. Timings include client/kernel scheduling;
one independent witness is a diagnostic, not an all-user tail-latency guarantee.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import traceback
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fallback_peers', ROOT / 'scripts/departure-fairness.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
wire = fixture.wire
TOKEN = 'fallback-admin'
PASSWORD = b'fallback-auth'


class BlockedPeer(fixture.Peer):
    def __init__(self, port, channel, index):
        self.received_data_bytes = 0
        self.socket = socket.socket()
        # Set before connect to prevent receive-window autotuning from absorbing
        # the whole echo. Other peers retain the ordinary diagnostic transport.
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
        self.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.socket.settimeout(5)
        self.socket.bind((f'127.0.{index // 12000}.2', 0))
        self.socket.connect(('127.0.0.1', port))
        self.socket.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n'
                            b'Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n'
                            b'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n')
        header = bytearray()
        while not header.endswith(b'\r\n\r\n'):
            header.extend(self.exact(1))
            assert len(header) < 8192, 'oversized upgrade response'
        assert header.startswith(b'HTTP/1.1 101 '), header
        self.socket.sendall(wire.frame(0x82, bytes([0x40 + len(channel)]) + channel))
        opcode, body = self.receive()
        self.id, end = wire.uint(body)
        assert opcode == 2 and end == len(body)


def request(admin, path='/metrics', method='GET'):
    req = urllib.request.Request(f'http://127.0.0.1:{admin}{path}', method=method,
                                 headers={'Authorization': f'Bearer {TOKEN}'})
    with urllib.request.urlopen(req, timeout=5) as response:
        body = response.read().decode()
        if method == 'POST':
            assert response.status == 204
            return
    return {fields[0]: fields[1] for line in body.splitlines()
            if not line.startswith('#') and len(fields := line.split()) == 2}


def run(args):
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    while admin == port:
        admin = wire.runtime.free_port()
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind',
               f'127.0.0.1:{admin}', '--workers', str(args.workers),
               '--max-connections', str(args.audience + 4),
               '--max-channel-members', str(args.audience + 1),
               '--max-message-bytes', '65536', '--queue-bytes', '65546',
               '--total-queue-bytes', '268435456', '--messages-per-second', '0',
               '--bytes-per-second', '0', '--ping-interval-secs', '0',
               '--idle-timeout-secs', '0', '--tcp-send-buffer-bytes', '16384',
               '--tcp-receive-buffer-bytes', '65536', '--shutdown-timeout-secs', '10']
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command + ['--pin-workers']
    result = {
        'passed': False, 'server_forced_kill': False, 'command': command,
        'audience': args.audience, 'clock': 'time.monotonic_ns', 'samples': [],
        'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'helper_sha256': {str(Path(p).relative_to(ROOT)): hashlib.sha256(Path(p).read_bytes()).hexdigest()
                          for p in [fixture.__file__, wire.__file__, wire.runtime.__file__]},
        'client_affinity': sorted(os.sched_getaffinity(0)),
        'sampling': 'one outstanding witness echo; 1ms wait after each reply',
        'healthy_notices': 0, 'shutdown_closes': 0, 'tcp_eofs': 0,
    }
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env.update(WAR_AUTH_PASSWORD=PASSWORD.decode(), WAR_ADMIN_TOKEN=TOKEN)
    peers, failures = [], []
    stop, ready = threading.Event(), threading.Event()
    thread = None
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                assert server.poll() is None, 'server startup failed'
                try:
                    request(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError('server readiness deadline')
            started = time.monotonic()

            def join(channel, blocked=False):
                assert time.monotonic() - started < 60, 'population setup deadline'
                cls = BlockedPeer if blocked else fixture.Peer
                peer = cls(port, channel, len(peers))
                peers.append(peer)
                return peer

            large = join(b'fallback-large')
            audience = [join(b'fallback-large') for _ in range(args.audience)]
            source = join(b'fallback-small', blocked=True)
            target = join(b'fallback-small')
            witness = join(b'fallback-probe')
            # Use a full-width ID so the echoed frame occupies exactly 65,536
            # application bytes, leaving less than one departure's local credit.
            assigned = 2**63 + 123
            for op, value, expected in [
                (0, bytes([0x40 + len(PASSWORD)]) + PASSWORD, b'\xf5'),
                (3, b'\x1b' + assigned.to_bytes(8, 'big'), b'\x1b' + assigned.to_bytes(8, 'big')),
            ]:
                source.socket.sendall(wire.frame(0x82, b'\x82\x00\x82' + bytes([op]) + value))
                assert source.receive() == (2, b'\x82\x00\x82' + bytes([op]) + expected)
            source.id = assigned
            assert len({p.id for p in peers}) == len(peers)
            result['setup_seconds'] = time.monotonic() - started
            result['before'] = request(admin)
            assert int(result['before']['war_queued_bytes']) == 0
            value = b'\x59' + (65523).to_bytes(2, 'big') + b'x' * 65523
            source.socket.sendall(wire.frame(0x82, source.packet(value)))
            for _ in range(200):
                metrics = request(admin)
                if int(metrics['war_queued_bytes']) == 65536:
                    break
                time.sleep(.005)
            else:
                raise AssertionError('could not retain exactly one full-size source echo')
            result['staged'] = metrics

            def probe():
                sequence = 0
                try:
                    while not stop.is_set():
                        value = b'\x1b' + sequence.to_bytes(8, 'big')
                        sent = time.monotonic_ns()
                        witness.socket.sendall(wire.frame(0x82, witness.packet(value)))
                        witness.verify(value)
                        result['samples'].append({'sent_ns': sent, 'received_ns': time.monotonic_ns()})
                        sequence += 1
                        ready.set()
                        stop.wait(.001)
                except Exception:
                    failures.append(traceback.format_exc())

            thread = threading.Thread(target=probe, daemon=True)
            thread.start()
            assert ready.wait(5), 'witness did not start'
            time.sleep(.02)
            result['large_close_sent_ns'] = time.monotonic_ns()
            large.close()
            result['large_close_ack_ns'] = time.monotonic_ns()
            result['disconnect_sent_ns'] = time.monotonic_ns()
            request(admin, f'/v1/sessions/{source.id}/disconnect', 'POST')
            result['disconnect_accepted_ns'] = time.monotonic_ns()
            target.departure(source.id)
            result['small_notice_ns'] = time.monotonic_ns()
            result['small_notice_verified'] = True
            # Wait for observable coverage, rather than assuming a fixed sleep
            # gives the witness a turn while the large room is still flushing.
            deadline = time.monotonic() + 5
            while not any(s['sent_ns'] >= result['small_notice_ns'] for s in result['samples']):
                assert time.monotonic() < deadline and not failures, 'missing post-event witness'
                time.sleep(.001)
            stop.set()
            thread.join(6)
            assert not thread.is_alive() and not failures, failures
            for peer in audience:
                peer.departure(large.id)
                result['healthy_notices'] += 1
            source.socket.close()
            target.close()
            witness.close()
            for _ in range(200):
                metrics = request(admin)
                if (int(metrics['war_sessions']) == args.audience
                        and int(metrics['war_connections']) == args.audience
                        and int(metrics['war_queued_bytes']) == 0):
                    break
                time.sleep(.01)
            else:
                raise AssertionError('population failed to quiesce')
            result['after'] = metrics
            for key in wire.ZERO_ERRORS:
                assert metrics[key] == result['before'][key], (key, metrics[key])
            # The administrator deliberately cancels the one blocked partial
            # write. It cannot safely append Close inside that WebSocket frame.
            assert int(metrics['war_aborted_writes_total']) == 1, metrics
            assert int(metrics['war_routing_copied_bytes_total']) == 0
            assert int(metrics['war_queued_bytes']) == 0
            result['small_notice_ms'] = (result['small_notice_ns'] - result['disconnect_sent_ns']) / 1e6
            result['worst_witness_ms'] = max((s['received_ns'] - s['sent_ns']) / 1e6 for s in result['samples'])
            result['process_after'] = wire.runtime.snapshot(server.pid)
            server.terminate()
            for peer in audience:
                opcode, body = peer.receive()
                assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1001
                result['shutdown_closes'] += 1
                assert peer.socket.recv(1) == b'', 'missing transport EOF'
                result['tcp_eofs'] += 1
            assert server.wait(timeout=12) == 0
            result['passed'] = True
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            stop.set()
            for peer in peers:
                peer.socket.close()
            if thread is not None:
                thread.join(6)
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=12)
                except subprocess.TimeoutExpired:
                    result['server_forced_kill'] = True
                    server.kill()
                    server.wait(timeout=3)
            result['server_exit'] = server.returncode
            if result['server_forced_kill'] or server.returncode != 0:
                result['passed'] = False
            log.seek(0)
            result['server_log'] = log.read().decode(errors='replace')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--audience', type=int, default=2048)
    parser.add_argument('--workers', type=int, default=8)
    parser.add_argument('--server-cpus', default='')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.audience < 1 or args.workers < 1:
        parser.error('audience and workers must be positive')
    with args.output.open('x') as output:
        result = run(args)
        json.dump(result, output, indent=2)
        output.write('\n')
    print(json.dumps({k: result.get(k) for k in [
        'passed', 'audience', 'small_notice_ms', 'worst_witness_ms',
        'healthy_notices', 'shutdown_closes', 'tcp_eofs', 'error']}))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
