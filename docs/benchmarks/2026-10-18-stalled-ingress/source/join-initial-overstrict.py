#!/usr/bin/env python3
"""Check absolute prejoin deadlines beside verified healthy traffic, on disposable ports."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import threading
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)
TOKEN = 'join-pressure-fixture'


def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def normal_close(peer):
    peer.socket.sendall(wire.wire.frame(0x88, b'\x03\xe8'))
    assert peer.receive() == (8, b'\x03\xe8')
    assert peer.socket.recv(1) == b''
    peer.socket.close()


def probe(peer, sequence):
    value = b'\x58\x40' + sequence.to_bytes(8, 'little') + bytes(range(56))
    packet = b'\x82\x1b' + peer.id.to_bytes(8, 'big') + value
    peer.socket.sendall(wire.wire.frame(0x82, packet))
    opcode, body = peer.receive()
    assert opcode == 2 and body[0] == 0x82
    sender, end = wire.wire.uint(body, 1)
    assert sender == peer.id and body[end:] == value, 'sender/body/FIFO mismatch'


def run(args):
    port, admin = free_port(), free_port()
    while admin == port:
        admin = free_port()
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
               '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4',
               '--max-connections', str(args.connections + 1), '--join-timeout-secs', '2',
               '--ping-interval-secs', '0', '--idle-timeout-secs', '0',
               '--messages-per-second', '0', '--bytes-per-second', '0']
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env['WAR_ADMIN_TOKEN'] = TOKEN
    record = {'passed': False, 'command': command, 'connections': args.connections,
              'waves': args.waves, 'ping': args.ping, 'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'wire_sha256': hashlib.sha256((ROOT / 'scripts/input-corpus.py').read_bytes()).hexdigest(),
              'events': [], 'scope': 'Finite plaintext prejoin admission/deadline test, not a latency benchmark or production capacity claim.'}
    def metrics():
        request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization': 'Bearer ' + TOKEN})
        start = time.monotonic()
        with urllib.request.urlopen(request, timeout=2) as response:
            result = {k: int(v) for k, v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_'))}
        return result, (time.monotonic() - start) * 1000
    def gate(name, connections):
        limit = time.monotonic() + 3
        while True:
            m, elapsed = metrics()
            if m['war_connections'] == connections and m['war_sessions'] == 1:
                break
            assert time.monotonic() < limit, (name, m)
            time.sleep(.01)
        assert m['war_channels'] == 2 and m['war_queued_bytes'] == m['war_variable_bytes'] == 0
        record['events'].append({'event': name, 'metrics': m, 'management_ms': elapsed})
    peers = []; witness = None; worker = None; stop = threading.Event(); failures = []
    with args.output.with_suffix('.server.log').open('w') as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(300):
                assert server.poll() is None
                try:
                    metrics(); break
                except OSError:
                    time.sleep(.01)
            else:
                raise TimeoutError('management startup')
            record['baseline'] = metrics()[0]
            witness = wire.Peer(port); witness.join(b'join-witness')
            def healthy():
                try:
                    sequence = 0
                    while not stop.is_set():
                        probe(witness, sequence); sequence += 1
                    record['healthy_verified'] = sequence
                except BaseException as error:
                    failures.append(repr(error))
            worker = threading.Thread(target=healthy); worker.start()
            for wave in range(args.waves):
                opened = {}
                for _ in range(args.connections):
                    start = time.monotonic(); peer = wire.Peer(port); opened[peer.socket] = (peer, start)
                    peers.append(peer)
                gate(f'wave-{wave + 1}-pending', args.connections + 1)
                assert record['events'][-1]['metrics']['war_join_rejected_total'] == wave * args.connections
                first_ping = time.monotonic(); next_ping = first_ping; pending = {}; received = []; pongs = 0; ping_sequence = 0
                while opened:
                    assert not failures and worker.is_alive(), failures
                    assert time.monotonic() - first_ping < 5, 'prejoin deadline did not retire peers'
                    if args.ping and time.monotonic() >= next_ping:
                        for stream in opened:
                            if stream not in pending:
                                payload = ping_sequence.to_bytes(8, 'little')
                                stream.sendall(wire.wire.frame(0x89, payload)); pending[stream] = payload
                        ping_sequence += 1; next_ping = time.monotonic() + .1
                    readable, _, _ = select.select(list(opened), [], [], .02)
                    for stream in readable:
                        peer, started = opened[stream]; opcode, body = peer.receive()
                        if opcode == 10:
                            assert args.ping and body == pending.pop(stream); pongs += 1
                        else:
                            assert (opcode, body) == (8, b'\x03\xf0Channel Join Timeout'), (opcode, body)
                            received.append(time.monotonic() - started)
                            stream.sendall(wire.wire.frame(0x88, body)); assert stream.recv(1) == b''
                            stream.close(); del opened[stream]; peers.remove(peer); pending.pop(stream, None)
                assert min(received) >= 1.9, 'join rejected before its configured deadline'
                if args.ping:
                    assert pongs >= args.connections, 'Ping path was not exercised for the full population'
                gate(f'wave-{wave + 1}-retired', 1)
                assert record['events'][-1]['metrics']['war_join_rejected_total'] == (wave + 1) * args.connections
                record['events'][-1].update(first_close_seconds=min(received), last_close_seconds=max(received), verified_pongs=pongs)
                recovery = wire.Peer(port); recovery.join(b'join-recovery'); probe(recovery, wave); normal_close(recovery)
                gate(f'wave-{wave + 1}-recovered', 1)
            stop.set(); worker.join(timeout=5); assert not worker.is_alive() and not failures, failures
            assert record['healthy_verified'] > 0
            normal_close(witness); witness = None
            limit = time.monotonic() + 3
            while True:
                m, _ = metrics()
                if m['war_connections'] == m['war_sessions'] == m['war_read_buffer_reserved_bytes'] == 0:
                    break
                assert time.monotonic() < limit
                time.sleep(.01)
            joined = args.waves + 1; rejected = args.connections * args.waves; messages = record['healthy_verified'] + args.waves
            expected = {'war_accepted_total': joined + rejected, 'war_joined_total': joined,
                        'war_disconnected_total': joined, 'war_join_rejected_total': rejected,
                        'war_incoming_messages_total': messages, 'war_incoming_bytes_total': messages * 76,
                        'war_deliveries_total': messages + joined, 'war_channels': 1}
            for key, value in expected.items():
                assert m[key] == value, (key, m[key], value)
            for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_output_queue_capacity_bytes',
                        'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total', 'war_rejected_total', 'war_handshake_errors_total',
                        'war_protocol_errors_total', 'war_close_failures_total', 'war_write_errors_total', 'war_write_timeouts_total',
                        'war_transport_disconnects_total', 'war_global_pressure_total', 'war_slow_consumers_total']:
                assert m[key] == 0, (key, m[key])
            record.update(passed=True, final_metrics=m, exact_checks=expected)
        except Exception as error:
            record['error'] = repr(error)
            raise
        finally:
            stop.set()
            for peer in peers:
                peer.socket.close()
            if witness:
                witness.socket.close()
            if worker:
                worker.join(timeout=6)
            if server.poll() is None:
                server.terminate()
            record['server_exit_code'] = server.wait(timeout=10)
            record['passed'] = record['passed'] and record['server_exit_code'] == 0
            args.output.write_text(json.dumps(record, indent=2) + '\n')
    return record


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--connections', type=int, default=128)
    parser.add_argument('--waves', type=int, default=2)
    parser.add_argument('--ping', action='store_true', help='Send checked Pings without joining; they must not extend the deadline')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.connections < 1 or args.waves < 1:
        parser.error('connections and waves must be positive')
    if args.output.exists():
        parser.error('use a fresh output path')
    result = run(args)
    print(json.dumps({k: result.get(k) for k in ['passed', 'healthy_verified', 'error']}))
