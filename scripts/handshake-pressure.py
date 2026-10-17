#!/usr/bin/env python3
"""Verify pending HTTP waves beside independently checked CBOR traffic.

Raw sockets either stay silent or send an incomplete header plus paced bytes.
Linux TCP receive queues establish a drained boundary between drip rounds.
Every pending socket must expire without application bytes at its absolute
handshake deadline. Admission, read reservations, management responsiveness,
subsequent slot reuse and final resource retirement are checked independently.
Optional gperftools captures replace the server allocator; those are diagnostics,
never ordinary timing results. --quiet holds the verified CBOR client at its
ready gate through the raw wave, then verifies its traffic and exact Close/EOF.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import selectors
import signal
import socket
import subprocess
import sys
import tempfile
import time

import benchmark as runtime
from inspection import dump_heap

PREFIX = (b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n'
          b'Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n'
          b'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n'
          b'Sec-WebSocket-Protocol: war.cbor.v1\r\nX-Padding: ')
HANDSHAKE_RESERVATION = 40960


def metrics(port):
    return {key: int(value) for key, value in runtime.metrics(port).items()}


def receive_queues(pid, port, peers=None):
    rows = Path(f'/proc/{pid}/net/tcp').read_text().splitlines()[1:]
    return [int(row[4].split(':')[1], 16) for row in (line.split() for line in rows)
            if row[3] == '01' and int(row[1].split(':')[1], 16) == port
            and (peers is None or int(row[2].split(':')[1], 16) in peers)]


def gate(client, name):
    assert select.select([client.stdout], [], [], 15)[0], f'missing {name} gate'
    line = client.stdout.readline()
    assert line, f'client stopped before {name}'
    event = json.loads(line)
    assert event['event'] == name, event
    return event['details']


def advance(client):
    client.stdin.write('continue\n'); client.stdin.flush()


def empty(observed):
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes']:
        assert observed[key] == 0, (key, observed[key])
    assert observed['war_channels'] == 1


def cpu_set(value):
    cpus = set()
    for part in value.split(','):
        bounds = part.split('-')
        cpus.update(range(int(bounds[0]), int(bounds[-1]) + 1))
    return cpus


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    assert port != admin
    env = {key: value for key, value in os.environ.items() if not key.startswith('WAR_') and key != 'LD_PRELOAD'}
    server_command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
                      '--workers', str(args.workers), '--max-connections', str(args.connections + 1),
                      '--handshake-timeout-secs', '5', '--ping-interval-secs', '0',
                      '--messages-per-second', '0', '--bytes-per-second', '0']
    client_command = [str(args.client.resolve()), '--url', f'ws://127.0.0.1:{port}', '--tcp', f'127.0.0.1:{port}',
                      '--flood-workers', '0', '--seconds', str(args.seconds)]
    if args.server_cpus: server_command = ['taskset', '-c', args.server_cpus] + server_command
    if args.client_cpus: client_command = ['taskset', '-c', args.client_cpus] + client_command
    if args.controller_cpus: os.sched_setaffinity(0, cpu_set(args.controller_cpus))
    server_env = dict(env)
    if args.heap_prefix:
        assert args.quiet and args.heap_library.is_file()
        args.heap_prefix.parent.mkdir(parents=True, exist_ok=True)
        assert not list(args.heap_prefix.parent.glob(args.heap_prefix.name + '*.heap')), 'use a fresh heap prefix'
        server_env.update(LD_PRELOAD=str(args.heap_library.resolve()), HEAPPROFILE=str(args.heap_prefix.resolve()),
                          HEAPPROFILESIGNAL=str(signal.SIGUSR2), HEAP_PROFILE_ALLOCATION_INTERVAL='0',
                          HEAP_PROFILE_INUSE_INTERVAL='0', HEAP_PROFILE_TIME_INTERVAL='0')
    record = {'scope': __doc__, 'args': {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              'server_command': server_command, 'client_command': client_command,
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'client_sha256': hashlib.sha256(args.client.read_bytes()).hexdigest(), 'source': Path(__file__).read_text(),
              'profile_diagnostic': bool(args.heap_prefix), 'events': [], 'samples': [], 'management': [], 'passed': False,
              'isolated_namespace': args.inside, 'network_namespace': os.readlink('/proc/self/ns/net')}
    if args.inside: assert record['network_namespace'] != args.host_netns
    if args.heap_prefix:
        record['heap_library'] = {'path': str(args.heap_library.resolve()), 'sha256': hashlib.sha256(args.heap_library.read_bytes()).hexdigest()}
    server = client = None
    raw = []; raw_peers = set()
    started = run_started = time.monotonic()

    def observe():
        stamp = time.monotonic(); observed = metrics(admin)
        record['management'].append({'seconds': time.monotonic() - run_started, 'latency_ms': (time.monotonic() - stamp) * 1000})
        return observed

    with tempfile.TemporaryFile() as log, tempfile.TemporaryFile() as client_log, selectors.DefaultSelector() as selector:
        try:
            server = subprocess.Popen(server_command, env=server_env, stdout=log, stderr=log)
            deadline = time.monotonic() + 5
            while True:
                assert server.poll() is None
                try: observe(); break
                except OSError:
                    assert time.monotonic() < deadline
                    time.sleep(.01)
            client = subprocess.Popen(client_command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=client_log, text=True, bufsize=1)
            welcome = gate(client, 'ready'); assert welcome['joins'] == 1

            def snapshot(name):
                item = {'event': name, 'seconds': time.monotonic() - started,
                        'metrics': observe(), 'process': runtime.snapshot(server.pid)}
                if args.heap_prefix: item['heap'] = dump_heap(server.pid, args.heap_prefix.resolve())
                record['events'].append(item)
                return item

            def drain(count):
                limit = time.monotonic() + 3
                previous = False
                while True:
                    queues = receive_queues(server.pid, port, raw_peers)
                    settled = len(queues) == count and not any(queues)
                    if settled and previous: return
                    assert time.monotonic() < limit, ('receive queues did not drain', len(queues), count)
                    previous = settled
                    time.sleep(.01)

            started = opened = time.monotonic()
            for _ in range(args.connections):
                stream = socket.create_connection(('127.0.0.1', port), timeout=3)
                stream.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                if args.kind == 'drip': stream.sendall(PREFIX)
                stream.setblocking(False); raw.append(stream); raw_peers.add(stream.getsockname()[1])
                selector.register(stream, selectors.EVENT_READ)
            deadline = time.monotonic() + 3
            while observe()['war_connections'] != args.connections + 1:
                assert time.monotonic() < deadline
                time.sleep(.01)
            drain(args.connections)
            with socket.create_connection(('127.0.0.1', port), timeout=3) as refused:
                assert refused.recv(1) == b'', 'a full admission must refuse without application bytes'
            base = snapshot('ready')
            assert base['metrics']['war_sessions'] == 1
            assert base['metrics']['war_rejected_total'] == 1
            allowance = base['metrics']['war_read_buffer_reserved_bytes']
            assert args.connections * HANDSHAKE_RESERVATION <= allowance <= args.connections * HANDSHAKE_RESERVATION + args.workers * 8192
            if not args.quiet: advance(client)
            round_start = time.monotonic()
            for number in range(args.rounds if args.kind == 'drip' else 0):
                stamp = round_start + number * args.period_ms / 1000
                if stamp > time.monotonic(): time.sleep(stamp - time.monotonic())
                for stream in raw: assert stream.send(b'x') == 1
                drain(args.connections)
                record['samples'].append({'round': number + 1, 'seconds': time.monotonic() - started,
                                          'metrics': observe()})
            dripped = snapshot('dripped')
            assert dripped['metrics']['war_connections'] == args.connections + 1
            assert dripped['metrics']['war_handshake_errors_total'] == 0
            record['sent_drip_bytes'] = args.connections * (args.rounds if args.kind == 'drip' else 0)
            closed = []
            expiry_limit = started + 10
            next_sample = time.monotonic()
            while selector.get_map():
                for key, _ in selector.select(.05):
                    try: data = key.fileobj.recv(1)
                    except (ConnectionResetError, ConnectionAbortedError): data = b''
                    assert data == b'', 'pending HTTP must expire without application bytes'
                    closed.append(time.monotonic() - opened); selector.unregister(key.fileobj); key.fileobj.close()
                assert server.poll() is None and client.poll() is None
                assert time.monotonic() < expiry_limit, 'pending HTTP failed to expire'
                if time.monotonic() >= next_sample:
                    observe(); next_sample = time.monotonic() + .2
            record['expiry_seconds'] = {'first': min(closed) if closed else None, 'last': max(closed) if closed else None}
            if closed: assert min(closed) >= 4.85, 'partial input ended before the five-second handshake deadline'
            deadline = time.monotonic() + 3
            while True:
                observed = observe()
                if observed['war_connections'] == 1 and observed['war_handshake_errors_total'] == args.connections: break
                assert time.monotonic() < deadline
                time.sleep(.01)
            expired = snapshot('expired')
            assert expired['metrics']['war_read_buffer_reserved_bytes'] <= args.workers * 8192

            recovery = min(args.connections, 16)
            renewed = [socket.create_connection(('127.0.0.1', port), timeout=3) for _ in range(recovery)]
            raw.extend(renewed)
            deadline = time.monotonic() + 3
            while observe()['war_connections'] != recovery + 1:
                assert time.monotonic() < deadline
                time.sleep(.01)
            reused = snapshot('reused')
            assert recovery * HANDSHAKE_RESERVATION <= reused['metrics']['war_read_buffer_reserved_bytes'] <= recovery * HANDSHAKE_RESERVATION + args.workers * 8192
            for stream in renewed: stream.close()
            if args.quiet: advance(client)
            measured = gate(client, 'measured')
            assert measured['healthy']['verified'] == measured['messages'] > 0
            record['healthy'] = measured
            measured_state = snapshot('measured')
            record['server_cpu_seconds'] = measured_state['process']['cpu_seconds'] - base['process']['cpu_seconds']
            advance(client); done = gate(client, 'done'); assert done['normal_closes'] == 1
            advance(client); stdout, _ = client.communicate(timeout=5)
            assert not stdout.strip() and client.returncode == 0
            expected = {'war_connections': 0, 'war_sessions': 0, 'war_channels': 1,
                        'war_accepted_total': args.connections + recovery + 2, 'war_rejected_total': 1,
                        'war_handshake_errors_total': args.connections + recovery, 'war_joined_total': 1,
                        'war_disconnected_total': 1, 'war_incoming_messages_total': measured['messages'],
                        'war_incoming_bytes_total': measured['message_bytes'], 'war_deliveries_total': measured['messages'] + 1,
                        'war_delivered_bytes_total': measured['message_bytes'] + welcome['welcome_bytes'],
                        'war_routing_reused_bytes_total': measured['messages'] * 1027, 'war_routing_copied_bytes_total': 0}
            deadline = time.monotonic() + 3
            while True:
                observed = observe()
                if all(observed[key] == value for key, value in expected.items()): break
                assert time.monotonic() < deadline, {key: (observed[key], value) for key, value in expected.items() if observed[key] != value}
                time.sleep(.01)
            empty(observed); record['final'] = snapshot('done'); record['exact_checks'] = expected
            for key in ['war_accept_errors_total', 'war_setup_errors_total', 'war_protocol_errors_total', 'war_close_failures_total',
                        'war_write_errors_total', 'war_write_timeouts_total', 'war_rate_limited_total', 'war_slow_consumers_total',
                        'war_transport_disconnects_total', 'war_global_pressure_total']:
                assert observed[key] == 0, (key, observed[key])
            record['passed'] = True
        except Exception as error:
            record['error'] = repr(error)
            if server is not None and server.poll() is None:
                record['failure_metrics'] = observe()
                record['failure_seconds'] = time.monotonic() - started
        finally:
            for stream in raw: stream.close()
            for process, label in [(client, 'client'), (server, 'server')]:
                if process is None: continue
                if process.poll() is None:
                    if label == 'client': process.kill(); record['client_forced_kill'] = True
                    else: process.send_signal(signal.SIGTERM)
                try: record[label + '_exit_code'] = process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill(); record[label + '_exit_code'] = process.wait(timeout=5); record[label + '_forced_kill'] = True
            record['passed'] &= record.get('client_exit_code') == record.get('server_exit_code') == 0
        log.seek(0); client_log.seek(0)
        record.update(server_log=log.read().decode(errors='replace'), client_log=client_log.read().decode(errors='replace'))
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=runtime.SERVER)
    parser.add_argument('--client', type=Path, default=runtime.ROOT / 'target/release/examples/admission')
    parser.add_argument('--workers', type=int, choices=[1, 4], default=1)
    parser.add_argument('--connections', type=int, default=1024, help='Pending raw TCP sockets in 0..=8192')
    parser.add_argument('--kind', choices=['idle', 'drip'], default='drip')
    parser.add_argument('--rounds', type=int, default=32, help='Paced byte rounds in 0..=32')
    parser.add_argument('--period-ms', type=int, default=50, help='Round spacing in 20..=100 milliseconds')
    parser.add_argument('--seconds', type=int, default=6, help='Healthy client phase in 1..=15 seconds')
    parser.add_argument('--quiet', action='store_true')
    parser.add_argument('--isolate', action='store_true', help='Fresh network/PID namespace using bubblewrap')
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--host-netns', help=argparse.SUPPRESS)
    parser.add_argument('--heap-prefix', type=Path)
    parser.add_argument('--heap-library', type=Path, default=Path('/usr/lib/libtcmalloc.so'))
    parser.add_argument('--server-cpus', default='0-3')
    parser.add_argument('--client-cpus', default='4')
    parser.add_argument('--controller-cpus', default='5-7')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    for name, low, high in [('connections', 0, 8192), ('rounds', 0, 32), ('period_ms', 20, 100), ('seconds', 1, 15)]:
        if not low <= getattr(args, name) <= high: parser.error(f'{name.replace("_", "-")} must be in {low}..={high}')
    if not args.quiet and args.seconds < 6: parser.error('ordinary healthy traffic must span the five-second pending timeout')
    if args.isolate and not args.inside:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        writable = {args.output.parent.resolve()}
        if args.heap_prefix:
            args.heap_prefix.parent.mkdir(parents=True, exist_ok=True); writable.add(args.heap_prefix.parent.resolve())
        command = ['bwrap', '--unshare-net', '--unshare-pid', '--die-with-parent', '--ro-bind', '/', '/']
        for path in sorted(writable): command += ['--bind', str(path), str(path)]
        command += ['--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--', sys.executable,
                    str(Path(__file__).resolve()), *sys.argv[1:], '--inside', '--host-netns', os.readlink('/proc/self/ns/net')]
        raise SystemExit(subprocess.run(command).returncode)
    result = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True); args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: result.get(key) for key in ['passed', 'error', 'expiry_seconds', 'server_cpu_seconds']}), flush=True)
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__': main()
