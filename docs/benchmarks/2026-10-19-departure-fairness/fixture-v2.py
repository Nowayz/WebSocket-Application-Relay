#!/usr/bin/env python3
"""Short departure-pressure diagnostic; no runtime instrumentation or global tuning.

Four events (singleton/large/large/singleton) share the same resident audience.
Probe RTT includes client scheduling and loopback/kernel time. It is not an
isolated relay-hop measurement or a statistical tail-latency certification.
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

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('departure_wire', ROOT / 'scripts/fragment-fairness.py')
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)


class Peer(wire.Wire):
    def __init__(self, port, channel, index):
        self.received_data_bytes = 0
        # Distinct loopback source addresses avoid the per-destination ephemeral
        # port ceiling at large populations; this does not alter global sysctls.
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=5,
                                              source_address=(f'127.0.{index//12000}.2', 0))
        self.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.socket.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n'
                            b'Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n'
                            b'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n')
        header = bytearray()
        while not header.endswith(b'\r\n\r\n'):
            part = self.socket.recv(1024)
            assert part, 'upgrade EOF'
            header.extend(part)
            assert len(header) < 8192, 'oversized upgrade response'
        assert header.startswith(b'HTTP/1.1 101 '), header
        self.socket.sendall(wire.frame(0x82, bytes([0x40 + len(channel)]) + channel))
        opcode, body = self.receive()
        self.id, end = wire.uint(body)
        assert opcode == 2 and end == len(body) and self.id not in (0, 2**64-1)

    def departure(self, departed):
        opcode, body = self.receive()
        assert opcode == 2 and body[0] == 0x82
        target, end = wire.uint(body, 1)
        sender, end = wire.uint(body, end)
        assert target == 2**64-1 and sender == departed and end == len(body), body.hex()


def run(args):
    result = {'passed': False, 'audience': args.audience, 'samples': [], 'events': [],
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'interpretation': 'loopback RTT including client scheduling; short diagnostic only'}
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind',
               f'127.0.0.1:{admin}', '--workers', '8', '--pin-workers', '--max-connections',
               str(args.audience + 12), '--max-channel-members', str(args.audience + 2),
               '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0',
               '--idle-timeout-secs', '0', '--shutdown-timeout-secs', '2',
               '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536']
    command = ['taskset', '-c', args.server_cpus] + command
    result['server_command'] = command
    stop = threading.Event()
    peers, probes, threads, failures = [], [], [], []
    origin = time.monotonic_ns()
    def stamp():
        return (time.monotonic_ns() - origin) / 1e6
    def probe(peer, index):
        try:
            sequence = 0
            while not stop.is_set():
                value = b'\x58\x40' + sequence.to_bytes(8, 'big') + b'p'*56
                sent = stamp()
                peer.socket.sendall(wire.frame(0x82, peer.packet(value)))
                peer.verify(value)
                result['samples'].append({'probe': index, 'sent_ms': sent, 'received_ms': stamp()})
                sequence += 1
                stop.wait(.001)
        except Exception:
            failures.append(traceback.format_exc())
    with tempfile.TemporaryFile() as log:
        env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    raise RuntimeError('server startup failed')
                try:
                    wire.runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError('server startup')
            result['worker_affinity'] = wire.runtime.pin_child_workers(server.pid, 8, True, apply=False)
            setup = time.monotonic()
            def join(channel):
                if time.monotonic() - setup > 60:
                    raise TimeoutError('60-second setup budget exceeded')
                peer = Peer(port, channel, len(peers))
                peers.append(peer)
                return peer
            audience = [join(b'dp-big') for _ in range(args.audience)]
            large = [join(b'dp-big'), join(b'dp-big')]
            controls = [join(b'dp-control-0'), join(b'dp-control-1')]
            probes = [join(f'dp-probe-{i}'.encode()) for i in range(8)]
            assert len({p.id for p in peers}) == len(peers), 'duplicate ID'
            result['setup_seconds'] = time.monotonic() - setup
            result['before'] = wire.runtime.metrics(admin)
            for i, peer in enumerate(probes):
                thread = threading.Thread(target=probe, args=(peer, i), daemon=True)
                threads.append(thread)
                thread.start()
            time.sleep(.2)
            for label, peer, recipients in [('control-1', controls[0], []),
                                             ('large-1', large[0], audience + [large[1]]),
                                             ('large-2', large[1], audience),
                                             ('control-2', controls[1], [])]:
                event = {'label': label, 'recipients': len(recipients), 'start_ms': stamp()}
                time.sleep(.15)
                event['close_sent_ms'] = stamp()
                peer.close()
                event['close_ack_ms'] = stamp()
                time.sleep(.15)
                event['end_ms'] = stamp()
                result['events'].append(event)
                for recipient in recipients:
                    recipient.departure(peer.id)
                assert not failures, failures
            stop.set()
            for thread in threads:
                thread.join(6)
                assert not thread.is_alive(), 'probe failed to stop'
            assert not failures, failures
            assert all(any(s['probe'] == i for s in result['samples']) for i in range(8))
            for peer in probes:
                peer.close()
            for _ in range(100):
                metrics = wire.runtime.metrics(admin)
                if int(metrics['war_sessions']) == args.audience and int(metrics['war_queued_bytes']) == 0:
                    break
                time.sleep(.01)
            result['after'] = metrics
            assert int(metrics['war_sessions']) == args.audience and int(metrics['war_queued_bytes']) == 0
            for name in wire.ZERO_ERRORS:
                assert metrics[name] == result['before'][name], (name, metrics[name])
            assert int(metrics['war_routing_copied_bytes_total']) == 0
            # Publish stopping before dropping the audience: prevent an O(N^2)
            # teardown storm from becoming part of this finite diagnostic.
            server.terminate()
            opcode, body = audience[0].receive()
            assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1001
            for peer in peers:
                peer.socket.close()
            result['server_exit'] = server.wait(timeout=6)
            assert result['server_exit'] == 0
            result['passed'] = True
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            stop.set()
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=6)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=3)
            for peer in peers:
                peer.socket.close()
            for thread in threads:
                thread.join(6)
            log.seek(0)
            result['server_log'] = log.read().decode(errors='replace')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--audience', type=int, default=8192)
    parser.add_argument('--server-cpus', default='0-7')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.audience <= 100000:
        parser.error('audience must be 1..100000 for this bounded diagnostic')
    # Reserve output before launching any process; preserve existing evidence.
    with args.output.open('x') as output:
        result = run(args)
        json.dump(result, output, indent=2)
        output.write('\n')
    print(json.dumps({k: result[k] for k in ('passed', 'audience', 'events')}, indent=2))
    if not result['passed']:
        print(result.get('error', result['server_log']))
        raise SystemExit(1)


if __name__ == '__main__':
    main()
