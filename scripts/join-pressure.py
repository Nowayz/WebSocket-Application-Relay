#!/usr/bin/env python3
"""Check absolute prejoin deadlines beside verified healthy traffic, on disposable ports."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import selectors
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
    if args.withhold_close_ack:
        command += ['--write-timeout-secs', '1']
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env['WAR_ADMIN_TOKEN'] = TOKEN
    record = {'passed': False, 'command': command, 'connections': args.connections,
              'waves': args.waves, 'ping': args.ping, 'withhold_close_ack': args.withhold_close_ack, 'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'wire_sha256': hashlib.sha256((ROOT / 'scripts/input-corpus.py').read_bytes()).hexdigest(),
              'fragment_wire_sha256': hashlib.sha256((ROOT / 'scripts/fragment-fairness.py').read_bytes()).hexdigest(),
              'verified_closing_refusals': 0, 'events': [], 'scope': 'Finite plaintext prejoin admission/deadline test, not a latency benchmark or production capacity claim.'}
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
        record['events'].append({'event': name, 'metrics': m, 'management_ms': elapsed})
        # The witness has one outstanding 76-byte envelope; its reservation may
        # be sampled during the synchronous write. Only final retirement is idle.
        assert m['war_channels'] == 2 and m['war_variable_bytes'] == 0, (name, m)
        assert m['war_queued_bytes'] <= 76, (name, m['war_queued_bytes'])
    recovered_bytes = 0
    peers = []; witness = None; worker = None; poller = None; stop = threading.Event(); failures = []
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
                poller = selectors.DefaultSelector()
                for _ in range(args.connections):
                    start = time.monotonic(); peer = wire.Peer(port); opened[peer.socket] = (peer, start)
                    peers.append(peer); poller.register(peer.socket, selectors.EVENT_READ)
                gate(f'wave-{wave + 1}-pending', args.connections + 1)
                assert record['events'][-1]['metrics']['war_join_rejected_total'] == wave * args.connections
                first_ping = time.monotonic(); next_ping = first_ping; pending = {}; closing = {}; close_to_eof = []; received = []; pongs = 0; pong_peers = set(); ping_sequence = 0
                while opened:
                    assert not failures and worker.is_alive(), failures
                    assert time.monotonic() - first_ping < 5, 'prejoin deadline did not retire peers'
                    if args.ping and time.monotonic() >= next_ping:
                        for stream in opened:
                            if stream not in pending and stream not in closing:
                                payload = ping_sequence.to_bytes(8, 'little')
                                stream.sendall(wire.wire.frame(0x89, payload)); pending[stream] = payload
                        ping_sequence += 1; next_ping = time.monotonic() + .1
                    for key, _ in poller.select(.02):
                        stream = key.fileobj
                        peer, started = opened[stream]
                        if stream in closing:
                            assert stream.recv(1) == b'', 'unexpected bytes after policy Close'
                            close_to_eof.append(time.monotonic() - closing.pop(stream))
                            poller.unregister(stream); stream.close(); del opened[stream]; peers.remove(peer)
                            continue
                        opcode, body = peer.receive()
                        if opcode == 10:
                            assert args.ping and body == pending.pop(stream); pongs += 1; pong_peers.add(stream)
                        else:
                            assert (opcode, body) == (8, b'\x03\xf0Channel Join Timeout'), (opcode, body)
                            received.append(time.monotonic() - started)
                            pending.pop(stream, None)
                            if args.withhold_close_ack:
                                closing[stream] = time.monotonic()
                                if len(received) == args.connections:
                                    gate(f'wave-{wave + 1}-closing', args.connections + 1)
                                    assert record['events'][-1]['metrics']['war_close_failures_total'] == wave * args.connections
                                    # Admission must still account for the real closing sockets.
                                    with socket.create_connection(('127.0.0.1', port), timeout=3) as refused:
                                        assert refused.recv(1) == b'', 'closing peers released admission too early'
                                    record['verified_closing_refusals'] += 1
                            else:
                                stream.sendall(wire.wire.frame(0x88, body)); assert stream.recv(1) == b''
                                poller.unregister(stream); stream.close(); del opened[stream]; peers.remove(peer)
                poller.close(); poller = None
                assert min(received) >= 1.9, 'join rejected before its configured deadline'
                if args.withhold_close_ack:
                    assert len(close_to_eof) == args.connections and min(close_to_eof) >= .9
                if args.ping:
                    assert len(pong_peers) == args.connections, 'Ping path was not exercised for the full population'
                gate(f'wave-{wave + 1}-retired', 1)
                assert record['events'][-1]['metrics']['war_join_rejected_total'] == (wave + 1) * args.connections
                record['events'][-1].update(first_close_seconds=min(received), last_close_seconds=max(received), verified_pongs=pongs, verified_pong_peers=len(pong_peers))
                if args.withhold_close_ack:
                    record['events'][-1].update(first_close_to_eof_seconds=min(close_to_eof), last_close_to_eof_seconds=max(close_to_eof))
                recovery = wire.Peer(port); recovery.join(b'join-recovery'); probe(recovery, wave); normal_close(recovery)
                recovered_bytes += recovery.received_data_bytes
                gate(f'wave-{wave + 1}-recovered', 1)
            stop.set(); worker.join(timeout=5); assert not worker.is_alive() and not failures, failures
            assert record['healthy_verified'] > 0
            normal_close(witness)
            record['verified_application_bytes'] = witness.received_data_bytes + recovered_bytes
            witness = None
            limit = time.monotonic() + 3
            while True:
                m, _ = metrics()
                if m['war_connections'] == m['war_sessions'] == m['war_read_buffer_reserved_bytes'] == 0:
                    break
                assert time.monotonic() < limit
                time.sleep(.01)
            joined = args.waves + 1; rejected = args.connections * args.waves; messages = record['healthy_verified'] + args.waves
            expected = {'war_accepted_total': joined + rejected + record['verified_closing_refusals'],
                        'war_rejected_total': record['verified_closing_refusals'], 'war_joined_total': joined,
                        'war_disconnected_total': joined, 'war_join_rejected_total': rejected,
                        'war_incoming_messages_total': messages, 'war_incoming_bytes_total': messages * 76,
                        'war_deliveries_total': messages + joined,
                        'war_delivered_bytes_total': record['verified_application_bytes'],
                        'war_routing_reused_bytes_total': messages * 66,
                        'war_close_failures_total': rejected if args.withhold_close_ack else 0, 'war_channels': 1}
            for key, value in expected.items():
                assert m[key] == value, (key, m[key], value)
            for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_output_queue_capacity_bytes',
                        'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total', 'war_handshake_errors_total',
                        'war_protocol_errors_total', 'war_write_errors_total', 'war_write_timeouts_total',
                        'war_transport_disconnects_total', 'war_global_pressure_total', 'war_slow_consumers_total']:
                assert m[key] == 0, (key, m[key])
            record.update(passed=True, final_metrics=m, exact_checks=expected)
        except Exception as error:
            record['error'] = repr(error)
            raise
        finally:
            stop.set()
            if poller:
                poller.close()
            for peer in peers:
                peer.socket.close()
            if witness:
                witness.socket.close()
            if worker:
                worker.join(timeout=6)
            if server.poll() is None:
                server.terminate()
            try:
                record['server_exit_code'] = server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill(); record['server_exit_code'] = server.wait(timeout=5)
                record['server_forced_kill'] = True
            record['passed'] = record['passed'] and record['server_exit_code'] == 0 and not record.get('server_forced_kill')
            args.output.write_text(json.dumps(record, indent=2) + '\n')
    return record


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--connections', type=int, default=128)
    parser.add_argument('--waves', type=int, default=2)
    parser.add_argument('--ping', action='store_true', help='Send checked Pings without joining; they must not extend the deadline')
    parser.add_argument('--withhold-close-ack', action='store_true', help='Verify slot retention then EOF at a one-second close deadline without acknowledging Close')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.connections < 1 or args.waves < 1:
        parser.error('connections and waves must be positive')
    if args.output.exists():
        parser.error('use a fresh output path')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    result = run(args)
    print(json.dumps({k: result.get(k) for k in ['passed', 'healthy_verified', 'error']}))
    raise SystemExit(0 if result['passed'] else 1)
