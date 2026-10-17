#!/usr/bin/env python3
"""Measure verified CBOR echo progress during policy-refused native TCP floods.

Fresh loopback processes only. Build --release --bins --example admission first.
Each raw TCP lifecycle must finish with EOF/reset and no application bytes.
The retained session verifies complete 1KiB cyclic self messages and exact Close/EOF.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import time

import benchmark as runtime


def metrics(port):
    return {key: int(value) for key, value in runtime.metrics(port).items()}


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    assert port != admin
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
               '--workers', str(args.workers), '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0']
    command += ['--max-connections', '1'] if args.policy == 'global' else ['--max-connections-per-ip', '1']
    client_command = [str(args.client.resolve()), '--url', f'ws://127.0.0.1:{port}', '--tcp', f'127.0.0.1:{port}',
                      '--seconds', str(args.seconds), '--flood-workers', str(args.flood_workers)]
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command
    if args.client_cpus:
        client_command = ['taskset', '-c', args.client_cpus] + client_command
    env = {key: value for key, value in os.environ.items() if not key.startswith('WAR_') and key != 'LD_PRELOAD'}
    result = {'timestamp_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'scope': __doc__,
              'server_command': command, 'client_command': client_command,
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'client_sha256': hashlib.sha256(args.client.read_bytes()).hexdigest(),
              'runner_source': Path(__file__).read_text(), 'args': {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              'events': [], 'passed': False}
    server = client = None
    with tempfile.TemporaryFile() as server_log, tempfile.TemporaryFile() as client_log:
        try:
            server = subprocess.Popen(command, env=env, stdout=server_log, stderr=server_log)
            deadline = time.monotonic() + 5
            while True:
                assert server.poll() is None, 'server stopped during startup'
                try:
                    metrics(admin)
                    break
                except OSError:
                    assert time.monotonic() < deadline, 'startup timeout'
                    time.sleep(.01)
            client = subprocess.Popen(client_command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=client_log, text=True, bufsize=1)
            started = time.monotonic()
            ready = measured = None
            for expected in ['ready', 'measured', 'done']:
                while not select.select([client.stdout], [], [], .1)[0]:
                    assert client.poll() is None, 'client stopped before a phase gate'
                    assert time.monotonic() - started < args.seconds + 30, 'phase deadline exceeded'
                line = client.stdout.readline()
                assert line, 'client ended before a phase gate'
                event = json.loads(line)
                assert event['event'] == expected, event
                details = event['details']
                if expected == 'ready':
                    assert details['joins'] == 1
                    ready = details
                if expected == 'measured':
                    measured = details
                    assert measured['healthy']['verified'] == measured['messages'] > 0
                    assert (measured['refusals']['verified'] > 0) == (args.flood_workers > 0)
                ended = expected == 'done'
                refused = 0 if measured is None else measured['refusals']['verified']
                messages = 0 if measured is None else measured['messages']
                body_bytes = 0 if measured is None else measured['message_bytes']
                check = {'war_accepted_total': 1 + refused, 'war_rejected_total': refused,
                         'war_ip_rejected_total': refused if args.policy == 'ip' else 0,
                         'war_joined_total': 1, 'war_disconnected_total': int(ended),
                         'war_connections': 0 if ended else 1, 'war_sessions': 0 if ended else 1,
                         'war_channels': 1 if ended else 2, 'war_queued_bytes': 0, 'war_variable_bytes': 0,
                         'war_incoming_messages_total': messages, 'war_incoming_bytes_total': body_bytes,
                         'war_deliveries_total': messages + 1, 'war_delivered_bytes_total': body_bytes + ready['welcome_bytes'],
                         'war_routing_reused_bytes_total': messages * 1027, 'war_routing_copied_bytes_total': 0,
                         'war_accept_errors_total': 0, 'war_setup_errors_total': 0, 'war_handshake_errors_total': 0,
                         'war_protocol_errors_total': 0, 'war_rate_limited_total': 0, 'war_slow_consumers_total': 0,
                         'war_close_failures_total': 0, 'war_transport_disconnects_total': 0,
                         'war_write_errors_total': 0, 'war_write_timeouts_total': 0,
                         'war_global_pressure_total': 0, 'war_output_queue_capacity_bytes': 0}
                # Close-to-EOF can precede the final guard instruction. Wait for
                # physical admissions and reservations to reach the phase gate.
                settled = time.monotonic() + 5
                while True:
                    observed = metrics(admin)
                    if all(observed[key] == value for key, value in check.items()):
                        break
                    assert time.monotonic() < settled, {key: (observed[key], value) for key, value in check.items() if observed[key] != value}
                    time.sleep(.005)
                allowance = observed['war_read_buffer_reserved_bytes']
                assert allowance == 0 if ended else allowance <= args.workers * 8192
                event.update(metrics=observed, exact_counter_checks=check, process=runtime.snapshot(server.pid), client_process=runtime.snapshot(client.pid))
                result['events'].append(event)
                if expected == 'measured':
                    first = result['events'][0]
                    measured['server_cpu_seconds'] = event['process']['cpu_seconds'] - first['process']['cpu_seconds']
                    measured['client_cpu_seconds'] = event['client_process']['cpu_seconds'] - first['client_process']['cpu_seconds']
                    measured['server_cpu_per_wall'] = measured['server_cpu_seconds'] / measured['elapsed_seconds']
                    result['mixed'] = measured
                client.stdin.write('continue\n'); client.stdin.flush()
            stdout, _ = client.communicate(timeout=5)
            assert not stdout.strip() and client.returncode == 0
            result['passed'] = True
        except Exception as error:
            result['error'] = repr(error)
        finally:
            if client is not None:
                if client.poll() is None:
                    client.kill()
                client.communicate(timeout=5)
                result['client_exit_code'] = client.returncode
            if server is not None:
                if server.poll() is None:
                    server.send_signal(signal.SIGTERM)
                try:
                    result['server_exit_code'] = server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill(); result['server_exit_code'] = server.wait(timeout=5)
            result['passed'] &= result.get('client_exit_code') == result.get('server_exit_code') == 0
        server_log.seek(0); client_log.seek(0)
        result.update(server_log=server_log.read().decode(errors='replace'), client_log=client_log.read().decode(errors='replace'))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=runtime.SERVER)
    parser.add_argument('--client', type=Path, default=runtime.ROOT / 'target/release/examples/admission')
    parser.add_argument('--policy', choices=['global', 'ip'], default='global')
    parser.add_argument('--workers', type=int, choices=[1, 4], default=1)
    parser.add_argument('--flood-workers', type=int, default=256)
    parser.add_argument('--seconds', type=int, default=6)
    parser.add_argument('--server-cpus', default='0-3')
    parser.add_argument('--client-cpus', default='4-7')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'passed': result['passed'], 'error': result.get('error'), 'mixed': result.get('mixed')}), flush=True)
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
