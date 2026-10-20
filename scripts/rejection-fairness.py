#!/usr/bin/env python3
"""Checked partial-fanout rejection with a single owner and independent witnesses.

One worker deliberately cannot drain recipient output during a publisher call.
The declared global budget fits a captured view but less than a full native turn
of 256-byte values. Every source must close 1013 after its first request. Queued
prefixes remain best effort; primary delivery, denied follow-up exclusion,
control replies, counters and every retained shutdown Close/EOF are checked.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('observer_messages', ROOT / 'scripts/observer-message-fairness.py')
messages = importlib.util.module_from_spec(spec)
spec.loader.exec_module(messages)
wire = messages.wire
PASSWORD = b'rejection'
WITNESSES = 4


def run(args):
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    while admin == port:
        admin = wire.runtime.free_port()
    population = args.audience + args.publishers + WITNESSES + 1
    # Fund one full source set plus one fixture frame per peer. Global credit
    # separately creates the intended publication failure, not a local overflow.
    queue_bytes = max(4106, (args.publishers + 1) * 269)
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
               '--admin-bind', f'127.0.0.1:{admin}', '--workers', '1',
               '--max-connections', str(population + 1),
               '--max-channels', str(args.publishers + WITNESSES + 3),
               '--max-channel-members', str(args.audience), '--max-message-bytes', '4096',
               '--queue-bytes', str(queue_bytes), '--total-queue-bytes', str(args.queue_bytes),
               '--ping-interval-secs', '0', '--idle-timeout-secs', '0',
               '--messages-per-second', '0', '--bytes-per-second', '0',
               '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536',
               '--shutdown-timeout-secs', '10']
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command + ['--pin-workers']
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env['WAR_AUTH_PASSWORD'] = PASSWORD.decode()
    helpers = [Path(messages.__file__), Path(messages.fixture.__file__), Path(wire.__file__), Path(wire.runtime.__file__)]
    result = {'passed': False, 'server_command': command,
              'args': {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'helper_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in helpers},
              'client_affinity': sorted(os.sched_getaffinity(0)),
              'scope': 'Finite one-worker rejection diagnostic. Partial prefixes are best effort, '
                       'not successful broadcasts. Every primary and retained observer is checked. '
                       'Timings include generator/kernel scheduling; no production latency guarantee.'}
    peers, observers, threads, failures, samples = [], [], [], [], []
    stopped = threading.Event()
    late_thread = None
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as output, tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        result['server_pid'] = server.pid
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

            def join(name):
                peer = messages.Peer(port, name, len(peers))
                peers.append(peer)
                return peer

            began = time.monotonic()
            sources = [join(f'rej-src{i:08x}'.encode()) for i in range(args.publishers)]
            target = join(b'rej-target')
            for _ in range(args.audience):
                if time.monotonic() - began > 60:
                    raise TimeoutError('60-second setup budget')
                peer = join(b're_globl')
                peer.request(messages.command(0, bytes([0x60 + len(PASSWORD)]) + PASSWORD), messages.command(0, b'\xf5'))
                peer.request(messages.command(1, b'\x02'), messages.command(1, b'\x02'))
                observers.append(peer)
            probes = [join(f'rej-probe{i:04x}'.encode()) for i in range(WITNESSES)]
            assert len({peer.id for peer in peers}) == population
            result['setup_seconds'] = time.monotonic() - began
            for _ in range(200):
                result['before'] = wire.runtime.metrics(admin)
                if int(result['before']['war_queued_bytes']) == 0:
                    break
                time.sleep(.01)
            assert int(result['before']['war_connections']) == population
            assert int(result['before']['war_queued_bytes']) == 0

            def probe(peer, index, ready):
                try:
                    while not stopped.is_set():
                        sent = time.monotonic_ns()
                        peer.request(messages.command(0, b'\x60'), messages.command(0, b'\xf4'))
                        samples.append({'probe': index, 'sent_ns': sent, 'received_ns': time.monotonic_ns()})
                        ready.set()
                        stopped.wait(.001)
                except Exception:
                    failures.append(traceback.format_exc())
                    ready.set()

            ready = [threading.Event() for _ in probes]
            for index, (peer, event) in enumerate(zip(probes, ready)):
                thread = threading.Thread(target=probe, args=(peer, index, event))
                threads.append(thread)
                thread.start()
            assert all(event.wait(2) for event in ready) and not failures, failures
            value = messages.byte_string_prefix(256) + b'r' * 256
            expected = {messages.packet(source.id, value) for source in sources}
            result['sent_ns'] = time.monotonic_ns()
            for source in sources:
                first = messages.packet(target.id, value)
                denied = messages.packet(target.id, b'\xf4')
                source.socket.sendall(wire.frame(0x82, first) + wire.frame(0x82, denied))
                # The second offered frame must never enter routing statistics.
                source.incoming += 1
                source.incoming_bytes += len(first)
                source.values += len(value)
            result['all_sent_ns'] = time.monotonic_ns()

            def late_join():
                try:
                    result['join_sent_ns'] = time.monotonic_ns()
                    late = join(b'rej-late')
                    result['join_welcome_ns'] = time.monotonic_ns()
                    late.finish()
                except Exception:
                    failures.append(traceback.format_exc())

            late_thread = threading.Thread(target=late_join)
            late_thread.start()
            for source in sources:
                opcode, body = source.receive()
                assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1013, 'source did not reject before follow-up'
                source.socket.sendall(wire.frame(0x88, body))
                assert source.socket.recv(1) == b'', 'rejection Close lacked EOF'
                source.closed = True
                source.socket.close()
            result['all_rejections_verified_ns'] = time.monotonic_ns()
            stopped.set()
            for thread in threads + [late_thread]:
                thread.join(timeout=6)
            assert all(not thread.is_alive() for thread in threads + [late_thread]) and not failures, failures
            target.expect_many(tuple(expected))
            result['primary_deliveries'] = args.publishers
            result['partial_observer_deliveries'] = 0
            for observer in observers:
                observer.send(messages.command(0, b'\x60'))
                seen = set()
                while True:
                    opcode, body = observer.receive()
                    assert opcode == 2, 'observer unexpectedly closed'
                    # Authentication is sticky after the successful setup.
                    if body == messages.command(0, b'\xf5'):
                        break
                    assert body in expected and body not in seen, {
                        'observer': str(observer.id), 'prefix': body[:48].hex(),
                        'length': len(body), 'duplicate': body in seen,
                        'expected_prefix': next(iter(expected))[:48].hex()}
                    seen.add(body)
                result['partial_observer_deliveries'] += len(seen)
            assert 0 < result['partial_observer_deliveries'] < args.publishers * args.audience
            for peer in [target, *probes]:
                peer.finish()
            exact = {'war_joined_total': len(peers),
                     'war_disconnected_total': sum(peer.closed for peer in peers),
                     'war_incoming_messages_total': sum(peer.incoming for peer in peers),
                     'war_incoming_bytes_total': sum(peer.incoming_bytes for peer in peers),
                     'war_deliveries_total': sum(peer.deliveries for peer in peers),
                     'war_delivered_bytes_total': sum(peer.received_data_bytes for peer in peers),
                     'war_routing_reused_bytes_total': sum(peer.values for peer in peers),
                     'war_routing_copied_bytes_total': 0,
                     'war_sessions': args.audience, 'war_connections': args.audience,
                     'war_queued_bytes': 0, 'war_variable_bytes': 0}
            for _ in range(200):
                metrics = wire.runtime.metrics(admin)
                if all(int(metrics[key]) == value for key, value in exact.items()):
                    break
                time.sleep(.01)
            result['after'] = metrics
            result['exact_metrics'] = exact
            for key, value in exact.items():
                assert int(metrics[key]) == value, (key, metrics[key], value)
            assert int(metrics['war_global_pressure_total']) == args.publishers
            for key in wire.ZERO_ERRORS:
                if key != 'war_global_pressure_total':
                    assert int(metrics[key]) == 0, (key, metrics[key])
            assert int(metrics['war_read_buffer_reserved_bytes']) <= 8192
            result['passed'] = True
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            stopped.set()
            for thread in threads + ([late_thread] if late_thread is not None else []):
                thread.join(timeout=6)
            result['probe_samples'] = samples
            result['probe_errors'] = failures
            if server.poll() is None:
                server.send_signal(signal.SIGTERM)
                result['shutdown_closes'] = 0
                try:
                    for peer in peers:
                        if not peer.closed:
                            opcode, body = peer.receive()
                            assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1001
                            assert peer.socket.recv(1) == b'', 'shutdown Close lacked EOF'
                            result['shutdown_closes'] += 1
                    result['shutdown_verified'] = True
                except Exception:
                    result['shutdown_error'] = traceback.format_exc()
                    result['passed'] = False
                try:
                    server.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=3)
                    result['forced_kill'] = True
            for peer in peers:
                peer.socket.close()
            result['server_exit'] = server.returncode
            if server.returncode != 0 or not result.get('shutdown_verified'):
                result['passed'] = False
            log.seek(0)
            result['server_log'] = log.read().decode(errors='replace')
            output.write(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'passed': result['passed'], 'output': str(args.output)}), flush=True)
    return result['passed']


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--audience', type=int, default=4096)
    parser.add_argument('--publishers', type=int, default=64)
    parser.add_argument('--queue-bytes', type=int, default=65536, help='Explicit global rejection-pressure credit')
    parser.add_argument('--server-cpus', default='')
    args = parser.parse_args()
    if min(args.audience, args.publishers) < 1:
        parser.error('audience and publishers must be positive')
    if args.queue_bytes < max(4106, (args.publishers + 1) * 269):
        parser.error('global credit must fund the declared local queue')
    raise SystemExit(0 if run(args) else 1)
