#!/usr/bin/env python3
"""Finite observer-message publication/revocation screen with independent witnesses.

Channel observers use private-echo witnesses. Private observers use control
replies: a private echo would itself fan out to the audience. Client/kernel time
is included. Shutdown verifies every retained Close/EOF without manufacturing
quadratic normal departures among global-room peers.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('departure', ROOT / 'scripts/departure-fairness.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
wire = fixture.wire
PASSWORD = b'observer-message'


def uint(value):
    return b'\x1b' + value.to_bytes(8, 'big')


def packet(sender, value):
    # The relay emits minimally encoded routing IDs, even when a request uses
    # an explicit uint64. Random IDs can occasionally fit a narrower width.
    if sender >= 2**32:
        encoded = uint(sender)
    elif sender < 24:
        encoded = bytes([sender])
    elif sender < 256:
        encoded = b'\x18' + sender.to_bytes(1, 'big')
    elif sender < 65536:
        encoded = b'\x19' + sender.to_bytes(2, 'big')
    else:
        encoded = b'\x1a' + sender.to_bytes(4, 'big')
    return b'\x82' + encoded + value


def command(index, value):
    return b'\x82\x00\x82' + bytes([index]) + value


class Peer(fixture.Peer):
    def __init__(self, *args):
        self.incoming = self.incoming_bytes = self.values = self.deliveries = 0
        self.closed = False
        super().__init__(*args)

    def receive(self):
        opcode, body = super().receive()
        self.deliveries += opcode == 2
        return opcode, body

    def send(self, body, value_bytes=0):
        self.socket.sendall(wire.frame(0x82, body))
        self.incoming += 1
        self.incoming_bytes += len(body)
        self.values += value_bytes

    def expect(self, body):
        opcode, received = self.receive()
        assert opcode == 2 and received == body, {
            'peer': str(self.id), 'opcode': opcode,
            'expected_prefix': body[:48].hex(), 'received_prefix': received[:48].hex()}

    def request(self, body, reply):
        self.send(body)
        self.expect(reply)

    def finish(self):
        self.socket.sendall(wire.frame(0x88, b'\x03\xe8'))
        assert self.receive() == (8, b'\x03\xe8'), 'normal Close changed'
        assert self.socket.recv(1) == b'', 'normal Close lacked transport EOF'
        self.closed = True
        self.socket.close()


def run(args):
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    while admin == port:
        admin = wire.runtime.free_port()
    population = args.audience + args.workers + 2
    server_command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
                      '--admin-bind', f'127.0.0.1:{admin}', '--workers', str(args.workers),
                      '--max-connections', str(population + 1),
                      '--max-channel-members', str(args.audience),
                      '--ping-interval-secs', '0', '--idle-timeout-secs', '0',
                      '--messages-per-second', '0', '--bytes-per-second', '0',
                      '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536',
                      '--shutdown-timeout-secs', '10']
    if args.server_cpus:
        server_command = ['taskset', '-c', args.server_cpus] + server_command + ['--pin-workers']
    if args.queue_messages is not None:
        server_command += ['--queue-messages', str(args.queue_messages)]
    environment = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    environment['WAR_AUTH_PASSWORD'] = PASSWORD.decode()
    profile = args.output.with_suffix('.perf.data') if args.profile else None
    if profile is not None and os.path.lexists(profile):
        raise FileExistsError(str(profile))
    result = {'passed': False, 'server_command': server_command,
              'args': {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'helper_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                               for p in [Path(fixture.__file__), Path(wire.__file__),
                                         Path(wire.runtime.__file__)]},
              'client_affinity': sorted(os.sched_getaffinity(0)), 'events': [],
              'probe_kind': 'private_echo' if args.kind == 'channel' else 'authentication_reply',
              'profile_diagnostic': args.profile,
              'scope': 'Finite native event screen; includes generator/kernel scheduling. '
                       'Every publication is checked, including before unsubscribe ACK, '
                       'immediate resubscription and subsequent windows. '
                       'Terminal shutdown checks every retained Close 1001 and EOF; '
                       'live gauges are checked before shutdown, not after process exit.'}
    peers, audience, threads, failures, samples = [], [], [], [], []
    stop = threading.Event()
    join_thread = None
    perf = None
    perf_log = None
    readable = None
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as output, tempfile.TemporaryFile() as log:
        server = subprocess.Popen(server_command, env=environment, stdout=log, stderr=log)
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
            setup = time.monotonic()

            def join(name):
                peer = Peer(port, name, len(peers))
                peers.append(peer)
                return peer

            flag = 1 if args.kind == 'channel' else 2
            source = join(b'om-source')
            target = join(b'om-target')
            for _ in range(args.audience):
                if time.monotonic() - setup > 60:
                    raise TimeoutError('60-second setup budget')
                peer = join(b're_globl')
                peer.request(command(0, bytes([0x60 + len(PASSWORD)]) + PASSWORD), command(0, b'\xf5'))
                peer.request(command(1, bytes([flag])), command(1, bytes([flag])))
                audience.append(peer)
            probes = [join(f'om-probe{i:04x}'.encode()) for i in range(args.workers)]
            assert len({peer.id for peer in peers}) == population
            result['setup_seconds'] = time.monotonic() - setup
            # SO_REUSEPORT distributes sessions independently of join order.
            # Wait for one ready socket without materializing an audience-sized
            # event list or accidentally choosing the busy publisher's worker.
            readable = select.epoll()
            readers_by_fd = {peer.socket.fileno(): peer for peer in audience}
            for descriptor in readers_by_fd:
                readable.register(descriptor, select.EPOLLIN)
            result['before'] = wire.runtime.metrics(admin)
            assert int(result['before']['war_sessions']) == population
            assert int(result['before']['war_connections']) == population
            if args.server_cpus:
                result['worker_affinity'] = wire.runtime.pin_child_workers(
                    server.pid, args.workers, True, apply=False)
            if profile is not None:
                perf_log = tempfile.TemporaryFile()
                perf = subprocess.Popen(['perf', 'record', '-e', 'cycles:u', '-F', '199',
                                         '--call-graph', 'dwarf,8192', '-p', str(server.pid),
                                         '-o', str(profile)], stdout=perf_log, stderr=perf_log)
                time.sleep(.05)
                assert perf.poll() is None, 'perf exited before the workload'
            result['process_before'] = wire.runtime.snapshot(server.pid)

            def probe(peer, index, ready):
                try:
                    sequence = 0
                    while not stop.is_set():
                        sent = time.monotonic_ns()
                        if args.kind == 'channel':
                            value = uint(sequence)
                            body = packet(peer.id, value)
                            peer.send(body, len(value))
                            peer.expect(body)
                        else:
                            peer.request(command(0, b'\x60'), command(0, b'\xf4'))
                        samples.append({'probe': index, 'sent_ns': sent,
                                        'received_ns': time.monotonic_ns()})
                        ready.set()
                        sequence += 1
                        stop.wait(.001)
                except Exception:
                    failures.append(traceback.format_exc())
                    ready.set()

            ready = [threading.Event() for _ in probes]
            for index, (peer, event) in enumerate(zip(probes, ready)):
                thread = threading.Thread(target=probe, args=(peer, index, event))
                threads.append(thread)
                thread.start()
            assert all(event.wait(2) for event in ready) and not failures, failures
            for sequence in range(args.rounds):
                value = b'\x59\x04\x00' + sequence.to_bytes(8, 'big') + b'm' * 1016
                expected = packet(source.id, value)
                event = {'sequence': sequence, 'sent_ns': time.monotonic_ns()}
                source.send(packet(2**64 - 1 if args.kind == 'channel' else target.id, value), len(value))
                first_ready = readable.poll(5, 1)
                assert first_ready and first_ready[0][1] & select.EPOLLIN, first_ready
                first_reader = readers_by_fd[first_ready[0][0]]
                first_reader.expect(expected)
                event['first_reader_ns'] = time.monotonic_ns()
                event['first_reader_id'] = str(first_reader.id)

                def independent_join():
                    try:
                        event['join_sent_ns'] = time.monotonic_ns()
                        late = join(b'om-late')
                        event['join_welcome_ns'] = time.monotonic_ns()
                        late.finish()
                        event['join_closed_ns'] = time.monotonic_ns()
                    except Exception:
                        failures.append(traceback.format_exc())

                join_thread = threading.Thread(target=independent_join)
                join_thread.start()
                observer = audience[-1]
                event['revoke_sent_ns'] = time.monotonic_ns()
                observer.send(command(1, b'\x00'))
                if observer is not first_reader:
                    observer.expect(expected)
                observer.expect(command(1, b'\x00'))
                event['revoke_ack_ns'] = time.monotonic_ns()
                # Reenable before draining the captured view. A deferred job
                # must not replay a message already flushed by this barrier.
                observer.request(command(1, bytes([flag])), command(1, bytes([flag])))
                event['resubscribe_ack_ns'] = time.monotonic_ns()
                join_thread.join(timeout=6)
                assert not join_thread.is_alive() and not failures, failures
                for reader in audience:
                    if reader is not first_reader and reader is not observer:
                        reader.expect(expected)
                if args.kind == 'private':
                    target.expect(expected)
                event['all_readers_verified_ns'] = time.monotonic_ns()
                result['events'].append(event)
            stop.set()
            for thread in threads:
                thread.join(timeout=6)
            assert all(not thread.is_alive() for thread in threads) and not failures, failures
            readable.close()
            readable = None
            result['process_after'] = wire.runtime.snapshot(server.pid)
            result['server_cpu_seconds'] = (result['process_after']['cpu_seconds']
                                            - result['process_before']['cpu_seconds'])
            for peer in [source, target, *probes]:
                peer.finish()
            expected_metrics = {
                'war_joined_total': len(peers),
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
                if all(int(metrics[key]) == count for key, count in expected_metrics.items()):
                    break
                time.sleep(.01)
            result['after'] = metrics
            for key, count in expected_metrics.items():
                assert int(metrics[key]) == count, (key, metrics[key], count)
            for key in wire.ZERO_ERRORS:
                assert int(metrics[key]) == 0, (key, metrics[key])
            assert int(metrics['war_read_buffer_reserved_bytes']) <= args.workers * 8192
            result['exact_metrics'] = expected_metrics
            result['passed'] = True
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            stop.set()
            for thread in threads + ([join_thread] if join_thread is not None else []):
                thread.join(timeout=6)
            if readable is not None:
                readable.close()
            result['probe_samples'] = samples
            result['probe_errors'] = failures
            if perf is not None:
                if perf.poll() is None:
                    perf.send_signal(signal.SIGINT)
                try:
                    perf.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    perf.kill()
                    perf.wait(timeout=3)
                    result['profile_forced_kill'] = True
                    result['passed'] = False
                perf_log.seek(0)
                result['profile_log'] = perf_log.read().decode(errors='replace')
                result['profile_exit'] = perf.returncode
                if perf.returncode not in (0, -signal.SIGINT):
                    result['passed'] = False
            if perf_log is not None:
                perf_log.close()
            if server.poll() is None:
                server.terminate()
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
                    result['server_forced_kill'] = True
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
    parser.add_argument('--audience', type=int, default=2048)
    parser.add_argument('--workers', type=int, default=2)
    parser.add_argument('--rounds', type=int, default=2)
    parser.add_argument('--kind', choices=['channel', 'private'], default='channel')
    parser.add_argument('--server-cpus', default='')
    parser.add_argument('--profile', action='store_true', help='Diagnostic server-only perf capture')
    parser.add_argument('--queue-messages', type=int, help='Explicit per-peer output-slot pressure diagnostic')
    arguments = parser.parse_args()
    if min(arguments.audience, arguments.workers, arguments.rounds) < 1:
        parser.error('audience, workers and rounds must be positive')
    if arguments.queue_messages is not None and arguments.queue_messages < 1:
        parser.error('queue messages must be positive')
    raise SystemExit(0 if run(arguments) else 1)
