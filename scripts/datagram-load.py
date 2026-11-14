#!/usr/bin/env python3
"""Gated verified open-loop private/fanout/mixed loads; separate submits and arrivals."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import tempfile
import time
import benchmark as runtime
import capacity
import inspection

ROOT = runtime.ROOT
GAUGES = ['war_sessions', 'war_connections', 'war_quic_connections', 'war_queued_bytes',
          'war_read_buffer_reserved_bytes', 'war_variable_bytes', 'war_output_queue_capacity_bytes']


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def run(args):
    if not (args.pairs >= 0 and args.readers >= 0 and args.pairs + args.readers > 0 and args.seconds > 0
            and args.drain_ms >= 0 and args.workers > 0 and args.client_workers > 0 and args.io_timeout_secs > 0):
        raise ValueError('invalid workload shape')
    if args.output.exists():
        raise FileExistsError(args.output)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    port, admin = runtime.free_port(), runtime.free_port()
    wt = args.mode != 'websocket'
    sessions = args.pairs * 2 + (args.readers + 1 if args.readers else 0)
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
               '--workers', str(args.workers), '--pin-workers', '--messages-per-second', '0', '--bytes-per-second', '0',
               '--ping-interval-secs', '0', '--max-connections', str(sessions),
               '--max-channels', str(args.pairs * 2 + 2), '--max-channel-members', str(max(1, args.readers + 1))]
    command += ['--dev-cert', '--udp-receive-buffer-bytes', str(args.udp_receive_buffer_bytes)] if wt else ['--legacy-websocket']
    command = ['taskset', '-c', args.server_cpus] + command
    client_command = [str(args.client.resolve()), '--url', f'https://127.0.0.1:{port}/war' if wt else f'ws://127.0.0.1:{port}',
                      '--mode', args.mode]
    for name in ['pairs', 'readers', 'private_rate', 'fanout_rate', 'seconds', 'drain_ms', 'payload_bytes',
                 'client_workers', 'udp_receive_buffer_bytes', 'io_timeout_secs']:
        client_command += ['--' + name.replace('_', '-'), str(getattr(args, name))]
    result = {'timestamp_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'args': {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              'host': {'uname': list(os.uname()), 'machine_before': capacity.machine()}, 'server_command': command, 'server_sha256': digest(args.server), 'client_sha256': digest(args.client),
              'source_sha256': {p: digest(ROOT / p) for p in ['examples/datagram_load.rs', 'examples/support/socket.rs', 'scripts/datagram-load.py']},
              'events': [], 'passed': False,
              'scope': 'Independent body/sender/sequence/generation-stamp checking at every recipient. Paced open loop; rates count verified unique arrivals during offer, with drain arrivals separate. Unreceived means within configured observation window, not attributed network loss. RSS is phase-gate sampling and excludes kernel storage. UDP queue/drop census is separately owned-inode scoped. Client generation lag/shortfall reported; this alone does not prove server saturation.'}
    environment = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    environment['RUST_LOG'] = 'war_relay=info'
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as errors:
        server = client = None
        ready = measured = None
        try:
            server = subprocess.Popen(command, stdout=logs, stderr=logs, env=environment)
            deadline = time.monotonic() + args.io_timeout_secs
            while True:
                if server.poll() is not None:
                    raise RuntimeError('server exited before readiness')
                try:
                    result['before'] = inspection.metrics(admin)
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(.02)
            if wt:
                import re
                text = os.pread(logs.fileno(), os.fstat(logs.fileno()).st_size, 0).decode()
                text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)  # colored logs, as in benchmark_transport
                match = re.search(r'certificate_sha256=([0-9a-fA-F:]{95})', text)
                if not match:
                    raise RuntimeError('development certificate pin missing')
                client_command += ['--certificate', match[1]]
            client_command = ['taskset', '-c', args.client_cpus] + client_command
            result['client_command'] = client_command
            client = subprocess.Popen(client_command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=errors, text=True, env=environment)
            deadline = time.monotonic() + args.timeout
            while True:
                if time.monotonic() >= deadline:
                    raise TimeoutError('load client exceeded controller watchdog')
                if not select.select([client.stdout], [], [], .25)[0]:
                    if client.poll() is not None:
                        raise RuntimeError('client exited without final gate')
                    continue
                line = client.stdout.readline()
                if not line:
                    raise RuntimeError('client EOF before final gate')
                event = json.loads(line)
                if event['event'] == 'done':
                    settle = time.monotonic() + args.io_timeout_secs
                    while True:
                        current = inspection.metrics(admin)
                        if all(current[k] == 0 for k in GAUGES):
                            break
                        if time.monotonic() >= settle:
                            raise RuntimeError('final resource gauges did not retire')
                        time.sleep(.02)
                else:
                    current = inspection.metrics(admin)
                server_inodes, client_inodes = set(), set()
                event.update(metrics=current, server=capacity.process(server.pid, server_inodes),
                             client=capacity.process(client.pid, client_inodes))
                event['server_udp'] = capacity.udp_census(server_inodes)
                event['client_udp'] = capacity.udp_census(client_inodes)
                result['events'].append(event)
                assert all(current[k] == result['before'][k] for k in inspection.ERRORS), 'server error/pressure counter changed'
                if event['event'] == 'ready':
                    ready = event
                    assert current['war_sessions'] == sessions
                    assert current['war_connections'] == sessions
                    assert current['war_quic_connections'] == (sessions if wt else 0)
                    result['thread_affinity'] = runtime.pin_child_workers(server.pid, args.workers, True, apply=False)
                    result['thread_affinity'] += runtime.pin_child_workers(client.pid, args.client_workers, False)
                elif event['event'] == 'measured':
                    measured = event
                    result['measurement'] = details = dict(event['details'])
                    elapsed = details['elapsed_seconds']
                    for role in ['server', 'client']:
                        seconds = event[role]['cpu_seconds'] - ready[role]['cpu_seconds']
                        details[role + '_cpu_seconds'] = seconds
                        details[role + '_cpu_cores_observation'] = seconds / elapsed
                        details[role + '_timed_udp_receive_drops'] = event[role + '_udp']['receive_drops'] - ready[role + '_udp']['receive_drops']
                    generated = sum(details[k]['generated_source_updates'] for k in ['private', 'fanout'])
                    verified = sum(details[k]['verified_unique'] for k in ['private', 'fanout'])
                    expected = sum(details[k]['expected_generated_deliveries'] for k in ['private', 'fanout'])
                    if args.mode == 'datagram':
                        assert current['war_incoming_messages_total'] == current['war_datagrams_received']
                        assert current['war_datagrams_received'] <= generated
                        assert verified <= current['war_datagrams_submitted'] <= expected
                        assert current['war_datagrams_oversized'] == 0
                    else:
                        assert current['war_incoming_messages_total'] == generated
                        assert verified == expected
                elif event['event'] == 'done':
                    assert measured and event['details']['passed']
                    assert event['details']['normal_closes'] == sessions
                    assert event['details']['source_departures_verified'] == args.readers
                    assert all(current[k] == 0 for k in GAUGES)
                else:
                    raise RuntimeError('unknown phase gate')
                client.stdin.write('continue\n'); client.stdin.flush()
                if event['event'] == 'done':
                    assert client.wait(timeout=args.io_timeout_secs) == 0
                    result['passed'] = True
                    break
        except Exception as error:
            result['error'] = repr(error)
        finally:
            if server is not None and server.poll() is None:
                try:
                    result['last_server_metrics'] = inspection.metrics(admin)
                    result['last_server_process'] = runtime.snapshot(server.pid)
                except Exception as diagnostic_error:
                    result['last_server_diagnostic_error'] = repr(diagnostic_error)
            if client is not None:
                if client.poll() is None:
                    client.kill(); client.wait()
                result['client_exit_code'] = client.returncode
                client.stdin.close()
                client.stdout.close()
            if server is not None:
                server.terminate()
                try:
                    result['server_exit_code'] = server.wait(timeout=args.io_timeout_secs)
                except subprocess.TimeoutExpired:
                    server.kill(); server.wait()
                    result['server_forced_kill'] = True
                if server.returncode != 0:
                    result['passed'] = False
                    result.setdefault('error', 'server failed clean shutdown')
            logs.seek(0); errors.seek(0)
            result['server_log'] = logs.read().decode(errors='replace')
            result['client_stderr'] = errors.read().decode(errors='replace')
            args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'passed': result['passed'], 'error': result.get('error'), 'output': str(args.output)}), flush=True)
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=ROOT / 'target/release/war-relay')
    parser.add_argument('--client', type=Path, default=ROOT / 'target/release/examples/datagram_load')
    parser.add_argument('--mode', choices=['websocket', 'stream', 'datagram'], required=True)
    for name, default in [('pairs', 32), ('readers', 0), ('private-rate', 1000), ('fanout-rate', 1000),
                          ('seconds', 4), ('drain-ms', 1000), ('payload-bytes', 1024), ('workers', 4),
                          ('client-workers', 8), ('udp-receive-buffer-bytes', 4194304), ('io-timeout-secs', 30), ('timeout', 180)]:
        parser.add_argument('--' + name, type=int, default=default)
    parser.add_argument('--server-cpus', default='0-3')
    parser.add_argument('--client-cpus', default='8-15')
    parser.add_argument('--output', type=Path, required=True)
    raise SystemExit(0 if run(parser.parse_args())['passed'] else 1)
