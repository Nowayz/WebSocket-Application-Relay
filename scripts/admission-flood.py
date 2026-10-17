#!/usr/bin/env python3
"""Measure verified CBOR echo progress during policy-refused native TCP floods.

Fresh loopback processes only. Build --release --bins --example admission first.
Each raw TCP lifecycle must finish with EOF/reset and no application bytes.
The retained session verifies complete 1KiB cyclic self messages and exact Close/EOF.
Echo and flood use independent processes and CPU sets; phase gates keep the
admitted socket alive until every refused lifecycle has retired.
--profile attaches perf only between ready/measured gates; those timings are
diagnostic and must be kept separate from ordinary comparisons.
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
import sys
import tempfile
import time

import benchmark as runtime


def metrics(port):
    return {key: int(value) for key, value in runtime.metrics(port).items()}


def tcp_counters():
    rows = Path('/proc/net/netstat').read_text().splitlines()
    values = dict(zip(rows[0].split()[1:], map(int, rows[1].split()[1:])))
    return {key: values[key] for key in ['ListenOverflows', 'ListenDrops', 'SyncookiesSent',
            'TCPReqQFullDoCookies', 'TCPReqQFullDrop', 'TCPSynRetrans']}


def listeners(port):
    try:
        lines = subprocess.check_output(['ss', '-H', '-ltn', f'sport = :{port}'], text=True).splitlines()
        return [{'queued': int(row[1]), 'backlog': int(row[2]), 'local': row[3]}
                for row in (line.split() for line in lines)]
    except (FileNotFoundError, subprocess.CalledProcessError):
        return []


def cpu_times(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    ticks = os.sysconf('SC_CLK_TCK')
    return {'user': int(fields[11]) / ticks, 'system': int(fields[12]) / ticks}


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    assert port != admin
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
               '--workers', str(args.workers), '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0']
    command += ['--max-connections', '1'] if args.policy == 'global' else ['--max-connections-per-ip', '1']
    client_base = [str(args.client.resolve()), '--url', f'ws://127.0.0.1:{port}', '--tcp', f'127.0.0.1:{port}',
                   '--seconds', str(args.seconds)]
    client_commands = {'healthy': client_base + ['--flood-workers', '0']}
    if args.flood_workers:
        client_commands['flood'] = client_base + ['--flood-only', '--flood-workers', str(args.flood_workers)]
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command
    for label, cpus in [('healthy', args.client_cpus), ('flood', args.flood_cpus)]:
        if cpus and label in client_commands:
            client_commands[label] = ['taskset', '-c', cpus] + client_commands[label]
    env = {key: value for key, value in os.environ.items() if not key.startswith('WAR_') and key != 'LD_PRELOAD'}
    result = {'timestamp_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'scope': __doc__,
              'server_command': command, 'client_commands': client_commands,
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'client_sha256': hashlib.sha256(args.client.read_bytes()).hexdigest(),
              'runner_source': Path(__file__).read_text(), 'args': {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              'profile_diagnostic': args.profile, 'events': [], 'passed': False}
    result['network_namespace'] = os.readlink('/proc/self/ns/net')
    result['isolated_namespace'] = bool(args.inside)
    if args.inside:
        assert result['network_namespace'] != args.host_netns
    result['tcp_counters_before'] = tcp_counters()
    server = perf = None
    clients = {}
    with tempfile.TemporaryFile() as server_log, tempfile.TemporaryFile() as client_log, tempfile.TemporaryFile() as perf_log:
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
            result['listener_queues_before'] = listeners(port)
            for label, client_command in client_commands.items():
                clients[label] = subprocess.Popen(client_command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                                 stderr=client_log, text=True, bufsize=1)
            started = time.monotonic()
            ready = measured = None
            for expected in ['ready', 'measured', 'done']:
                participants = {}
                for label, client in clients.items():
                    while not select.select([client.stdout], [], [], .1)[0]:
                        assert client.poll() is None, f'{label} stopped before a phase gate'
                        assert time.monotonic() - started < args.seconds + 30, 'phase deadline exceeded'
                    line = client.stdout.readline()
                    assert line, f'{label} ended before a phase gate'
                    item = json.loads(line)
                    assert item['event'] == expected, item
                    participants[label] = item['details']
                details = dict(participants['healthy'])
                event = {'event': expected, 'details': details, 'client_details': participants}
                if expected == 'ready':
                    assert details['joins'] == 1
                    ready = details
                if expected == 'measured':
                    details['healthy_elapsed_seconds'] = details['elapsed_seconds']
                    if 'flood' in participants:
                        raw = participants['flood']
                        assert raw['messages'] == raw['healthy']['verified'] == 0
                        details['refusals'] = raw['refusals']
                        details['flood_elapsed_seconds'] = raw['elapsed_seconds']
                        details['elapsed_seconds'] = max(details['elapsed_seconds'], raw['elapsed_seconds'])
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
                assert 0 <= allowance <= (0 if ended else args.workers * 8192)
                event.update(metrics=observed, exact_counter_checks=check, process=runtime.snapshot(server.pid),
                             server_cpu_times=cpu_times(server.pid),
                             client_processes={label: runtime.snapshot(client.pid) for label, client in clients.items()})
                result['events'].append(event)
                if expected == 'ready' and args.profile:
                    profile = args.output.with_suffix('.perf.data').resolve()
                    assert not profile.exists(), 'use a fresh profile output path'
                    perf_command = ['perf', 'record', '-e', 'cycles:u', '-F', '499', '--call-graph', 'dwarf,8192',
                                    '-p', str(server.pid), '-o', str(profile)]
                    perf = subprocess.Popen(perf_command, stdout=perf_log, stderr=perf_log)
                    time.sleep(.2)
                    assert perf.poll() is None, 'perf could not attach'
                    result.update(profile_path=str(profile), perf_command=perf_command)
                if expected == 'measured':
                    if perf is not None:
                        perf.send_signal(signal.SIGINT)
                        result['perf_exit_code'] = perf.wait(timeout=10)
                        perf = None
                        assert result['perf_exit_code'] in (0, -signal.SIGINT), 'perf failed'
                        result['profile_sha256'] = hashlib.sha256(profile.read_bytes()).hexdigest()
                    first = result['events'][0]
                    measured['server_cpu_seconds'] = event['process']['cpu_seconds'] - first['process']['cpu_seconds']
                    measured['server_cpu_times'] = {key: value - first['server_cpu_times'][key]
                                                    for key, value in event['server_cpu_times'].items()}
                    measured['client_cpu_by_role'] = {label: event['client_processes'][label]['cpu_seconds'] - first['client_processes'][label]['cpu_seconds'] for label in clients}
                    measured['client_cpu_seconds'] = sum(measured['client_cpu_by_role'].values())
                    measured['server_cpu_per_wall'] = measured['server_cpu_seconds'] / measured['elapsed_seconds']
                    result['mixed'] = measured
                for client in clients.values():
                    client.stdin.write('continue\n'); client.stdin.flush()
            for client in clients.values():
                stdout, _ = client.communicate(timeout=5)
                assert not stdout.strip() and client.returncode == 0
            result['passed'] = True
        except Exception as error:
            result['error'] = repr(error)
            if server is not None and server.poll() is None:
                result['failure_metrics'] = metrics(admin)
                result['listener_queues_at_failure'] = listeners(port)
        finally:
            if perf is not None:
                if perf.poll() is None:
                    perf.send_signal(signal.SIGINT)
                try:
                    result['perf_exit_code'] = perf.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    perf.kill(); result['perf_exit_code'] = perf.wait(timeout=5)
                    result['perf_forced_kill'] = True
            result['client_exit_codes'] = {}
            for label, client in clients.items():
                if client.poll() is None:
                    client.kill()
                client.communicate(timeout=5)
                result['client_exit_codes'][label] = client.returncode
            result['client_exit_code'] = next((code for code in result['client_exit_codes'].values() if code), 0)
            if server is not None:
                if server.poll() is None:
                    server.send_signal(signal.SIGTERM)
                try:
                    result['server_exit_code'] = server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill(); result['server_exit_code'] = server.wait(timeout=5)
            result['passed'] &= result.get('client_exit_code') == result.get('server_exit_code') == 0
        result['tcp_counters_after'] = tcp_counters()
        result['tcp_counter_delta'] = {key: value - result['tcp_counters_before'][key]
                                       for key, value in result['tcp_counters_after'].items()}
        server_log.seek(0); client_log.seek(0); perf_log.seek(0)
        result.update(server_log=server_log.read().decode(errors='replace'), client_log=client_log.read().decode(errors='replace'),
                      perf_log=perf_log.read().decode(errors='replace'))
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
    parser.add_argument('--client-cpus', default='4', help='Established echo client CPU set')
    parser.add_argument('--flood-cpus', default='5-7', help='Independent flood client CPU set')
    parser.add_argument('--isolate', action='store_true', help='Fresh private network namespace using bubblewrap')
    parser.add_argument('--profile', action='store_true', help='Diagnostic only: sample server user cycles with perf between phase gates')
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--host-netns', help=argparse.SUPPRESS)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.isolate and not args.inside:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        command = ['bwrap', '--unshare-net', '--unshare-pid', '--die-with-parent', '--ro-bind', '/', '/',
                   '--bind', str(args.output.parent.resolve()), str(args.output.parent.resolve()),
                   '--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp', '--', sys.executable,
                   str(Path(__file__).resolve()), *sys.argv[1:], '--inside', '--host-netns', os.readlink('/proc/self/ns/net')]
        raise SystemExit(subprocess.run(command).returncode)
    result = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'passed': result['passed'], 'error': result.get('error'), 'mixed': result.get('mixed')}), flush=True)
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
