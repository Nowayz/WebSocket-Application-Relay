#!/usr/bin/env python3
"""Gated population fairness with checked room multicast, churn and inspection.

Two clients use disjoint CPU sets and disjoint rooms. Capacity is held connected
until sidecar inspection and cleanup finish, preserving its exact expected census.
All server timing excludes setup, CSV output and teardown. Never time in parallel.
Overall success requires correctness and any explicitly offered fanout rate.
The receipt retains correctness_passed separately when the offered rate is missed.
"""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import time
import urllib.request

import benchmark as runtime

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def snapshot(pid, *, include_fds=True):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    status = Path(f'/proc/{pid}/status').read_text().splitlines()
    return {'cpu_seconds': (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'),
            'rss_kib': int(next(line.split()[1] for line in status if line.startswith('VmRSS:'))),
            'fds': len(list(Path(f'/proc/{pid}/fd').iterdir())) if include_fds else None}


def generator_lag(timeline, messages_per_second):
    """Expose sender backlog using the workload's own period, not an arbitrary SLO."""
    if type(messages_per_second) is not int or messages_per_second <= 0:
        raise ValueError('messages_per_second must be a positive integer')
    windows = timeline['windows']
    if not windows:
        raise ValueError('sender timeline must contain windows')
    for second, window in enumerate(windows):
        if type(window['intended_second']) is not int or window['intended_second'] != second:
            raise ValueError('sender timeline seconds must be contiguous ordered integers')
        delay = window['pre_send_delay']['max_ms']
        if type(delay) not in (int, float) or not math.isfinite(delay) or delay < 0:
            raise ValueError(f'invalid sender delay in second {second}')
    period_ms = 1000 / messages_per_second
    late = [w['intended_second'] for w in windows
            if w['pre_send_delay']['max_ms'] > period_ms]
    return {'available': True, 'period_ms': period_ms,
            'maximum_pre_send_ms': max(w['pre_send_delay']['max_ms'] for w in windows),
            'windows_over_one_period': late, 'exceeded_one_period': bool(late)}


def fanout_timing_summary(timing):
    """Summarize existing controller clocks after timing; never call this a server hop."""
    if timing is None:
        return {'available': False, 'reason': 'Client did not emit controller timing'}
    ready = timing['ready_ns']
    windows = timing['windows']
    if type(ready) is not int or ready < 0 or not windows:
        raise ValueError('invalid fanout controller readiness or empty windows')
    fields = ['scheduled_ns', 'wake_ns', 'sent_ns', 'acknowledged_ns', 'completed_ns']
    stages = {'wake_lag': [], 'publisher_work': [], 'after_last_send_to_all_acks': [],
              'membership_cleanup': [], 'scheduled_to_complete': []}
    previous_complete = ready
    for index, window in enumerate(windows):
        clocks = [window[key] for key in fields]
        if (any(type(t) is not int or t < 0 for t in clocks)
                or clocks != sorted(clocks) or clocks[1] < previous_complete):
            raise ValueError(f'nonmonotonic fanout controller window {index}')
        for name, value in zip(stages, [clocks[1]-clocks[0], clocks[2]-clocks[1],
                                       clocks[3]-clocks[2], clocks[4]-clocks[3],
                                       clocks[4]-clocks[0]]):
            stages[name].append(value)
        previous_complete = clocks[4]
    summary = {'available': True, 'windows': len(windows), 'controller_ready_ms': ready/1e6,
               'scope': 'Controller clocks. After-last-send includes relay, kernel, readers and acknowledgement processing; not server-hop latency.',
               'median_method': 'nearest rank', 'stages': {}}
    for name, values in stages.items():
        worst = max(range(len(values)), key=values.__getitem__)
        summary['stages'][name] = {'first_ms': values[0]/1e6,
                                  'median_ms': sorted(values)[(len(values)-1)//2]/1e6,
                                  'max_ms': values[worst]/1e6, 'max_window_index': worst}
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--kind', choices=['rust', 'cpp'], default='rust')
    parser.add_argument('--sidecar', type=Path, default=ROOT / 'target/release/examples/inspection')
    parser.add_argument('--capacity-client', type=Path, default=ROOT / 'artifacts/candidates/latency-timeline-client/capacity')
    parser.add_argument('--users', type=int, default=100000)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--readers', type=int, default=8192)
    parser.add_argument('--value-kind', choices=['bytes', 'ascii', 'utf8'], default='bytes',
                        help='Sidecar private/fanout value type; capacity probes remain byte strings')
    parser.add_argument('--scope', choices=['room', 'observers'], default='observers')
    parser.add_argument('--window', type=int, default=4)
    parser.add_argument('--inspectors', type=int, default=2)
    parser.add_argument('--http-page-size', type=int, help='Opt-in native HTTP first-page inspection, limit 1..=1000; full traversal checked outside timing')
    parser.add_argument('--pairs', type=int, default=1)
    parser.add_argument('--churn-workers', type=int, default=8,
                        help='Lifecycle workers; 0 disables churn (default: 8)')
    parser.add_argument('--churn-cycles-per-second', type=int, default=0,
                        help='Per-worker lifecycle rate; 0 means unpaced when workers are enabled')
    parser.add_argument('--fanout-windows-per-second', type=int, default=0)
    parser.add_argument('--fanout-churn-every', type=int, default=0)
    parser.add_argument('--exact-fanout-latency', action='store_true')
    parser.add_argument('--latency-stages', action='store_true', help='Opt-in client clock decomposition; diagnostic timing only')
    parser.add_argument('--pin-client-workers', action='store_true', help='Pin client I/O workers individually inside their existing CPU masks')
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--profile-target', choices=['server', 'capacity', 'sidecar'], default='server',
                        help='Process sampled by --profile (default: server); diagnostic timing only')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert args.users % 16 == 0 and args.seconds >= 2
    if args.profile_target != 'server' and not args.profile:
        parser.error('--profile-target requires --profile')
    if not 0 <= args.churn_workers <= 64:
        parser.error('churn-workers must be in 0..=64, matching the inspection client')
    if args.http_page_size is not None and not (
            args.kind == 'rust' and 1 <= args.http_page_size <= 1000 and not args.fanout_churn_every):
        parser.error('--http-page-size requires Rust, limit 1..=1000, and no fanout membership churn')
    prefix = args.output.with_suffix('')
    artifacts = [args.output, Path(str(prefix) + '.csv'), Path(str(prefix) + '.windows.json')]
    artifacts += [Path(str(prefix) + f'-{name}.err') for name in ('server', 'capacity', 'sidecar')]
    if args.exact_fanout_latency:
        artifacts.append(Path(str(prefix) + '-fanout.csv'))
    if args.profile:
        artifacts += [Path(str(prefix) + '.perf.data'), Path(str(prefix) + '-perf.log')]
    for artifact in artifacts:
        if os.path.lexists(artifact):
            parser.error(f'output already exists; choose a fresh artifact prefix: {artifact}')
    client_path = args.capacity_client.resolve()
    port, admin = free_port(), free_port()
    while admin == port:
        admin = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    # The HTTP inspection client uses this fixed token only for disposable fixtures.
    admin_token = 'inspection-benchmark' if args.http_page_size is not None else 'mixed-capacity-fixture'
    env.update(WAR_ADMIN_TOKEN=admin_token, WAR_AUTH_PASSWORD='inspection-benchmark')
    cmd = ['taskset', '-c', '0-7', str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
           '--admin-bind', f'127.0.0.1:{admin}', '--workers', '8', '--pin-workers',
           '--max-connections', str(args.users + args.readers + 1000),
           '--max-channel-members', str(max(10000, args.readers + 1)),
           '--max-message-bytes', '65536', '--queue-bytes', '262144',
           '--total-read-buffer-bytes', '268435456',
           '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0',
           '--idle-timeout-secs', '0', '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536']
    if args.kind == 'cpp':
        for option in ['--total-read-buffer-bytes', '--messages-per-second', '--bytes-per-second', '--ping-interval-secs', '--idle-timeout-secs']:
            position = cmd.index(option)
            del cmd[position:position + 2]
    cc = ['taskset', '-c', '8-11,24-27', str(client_path), '--url', f'ws://127.0.0.1:{port}',
          '--users', str(args.users), '--seconds', str(args.seconds), '--messages-per-second', '4',
          '--client-workers', '8', '--user-latency-output', str(prefix) + '.csv']
    if args.latency_stages:
        cc += ['--latency-stages']
    sc = ['taskset', '-c', '12-15,28-31', str(args.sidecar.resolve()), '--url', f'ws://127.0.0.1:{port}',
          '--protocol-only', '--mode', 'cbor', '--rooms', '0', '--pairs', str(args.pairs), '--seconds', str(args.seconds),
          '--churn-workers', str(args.churn_workers), '--churn-cycles-per-second', str(args.churn_cycles_per_second), '--inspectors', str(args.inspectors), '--client-workers', '8',
          '--fanout-readers', str(args.readers), '--fanout-window', str(args.window), '--fanout-global-observers',
          '--external-capacity-rooms', str(args.users // 16), '--fanout-close-window', '32', '--fanout-windows-per-second', str(args.fanout_windows_per_second)]
    if args.value_kind != 'bytes':
        sc += ['--value-kind', args.value_kind]
    if args.http_page_size is not None:
        sc.remove('--protocol-only')
        sc[sc.index('--mode') + 1] = 'http'
        sc += ['--admin', f'127.0.0.1:{admin}', '--page-size', str(args.http_page_size)]
    if args.fanout_churn_every:
        assert args.scope == 'room' and args.inspectors == 0
        sc[sc.index('--mode') + 1] = 'none'
        sc += ['--fanout-churn-every', str(args.fanout_churn_every)]
    if args.exact_fanout_latency:
        sc += ['--fanout-latency-output', str(prefix) + '-fanout.csv']
    if args.scope == 'room':
        sc.remove('--fanout-global-observers')
    record = {'passed': False, 'correctness_passed': False, 'args': vars(args).copy(), 'server_command': cmd, 'client_command': cc,
              'sidecar_command': sc, 'sha256': {'server': digest(args.server), 'capacity_client': digest(client_path),
              'sidecar': digest(args.sidecar), 'runner': digest(__file__), 'benchmark_helper': digest(runtime.__file__)}, 'events': [], 'sidecar_events': [], 'monitoring': [], 'client_thread_affinity': []}
    record['args'] = {k: str(v) if isinstance(v, Path) else v for k, v in record['args'].items()}
    record['cpu_topology'] = subprocess.check_output(['lscpu', '-p=CPU,CORE,SOCKET,NODE'], text=True)
    def cores(cpus):
        return {Path(f'/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list').read_text().strip() for cpu in cpus}
    assert not (cores(range(8)) & cores(list(range(8, 16)) + list(range(24, 32)))), 'client shares a server core'
    processes, files = [], []
    def launch(command, name):
        err = open(str(prefix) + f'-{name}.err', 'w'); files.append(err)
        proc = subprocess.Popen(command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=err, text=True, bufsize=1)
        processes.append(proc)
        return proc
    def metrics():
        request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization': f'Bearer {admin_token}'})
        with urllib.request.urlopen(request, timeout=5) as response:
            return {k: int(v) for k, v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_'))}
    def resume(proc):
        proc.stdin.write('continue\n'); proc.stdin.flush()
    def event(proc, collection):
        assert select.select([proc.stdout], [], [], 1200)[0], 'client event deadline'
        line = proc.stdout.readline()
        assert line, f'client stopped: {proc.poll()}'
        value = json.loads(line)
        value['controller_monotonic_seconds'] = time.monotonic()
        record[collection].append(value)
        args.output.write_text(json.dumps(record, indent=2) + '\n')
        print(json.dumps({'client': collection, 'event': value['event']}), flush=True)
        return value
    def verify_client_affinity():
        for pinned in record['client_thread_affinity']:
            assert os.sched_getaffinity(pinned['tid']) == {pinned['cpu']}, 'client worker affinity changed'

    perf = None
    try:
        server = launch(cmd, 'server')
        for _ in range(500):
            assert server.poll() is None, 'server stopped during startup'
            try:
                record['baseline_metrics'] = metrics(); break
            except OSError:
                time.sleep(.01)
        else:
            raise TimeoutError('startup')
        record['baseline'] = snapshot(server.pid)
        capacity = launch(cc, 'capacity')
        assert event(capacity, 'events')['event'] == 'connected'
        if args.pin_client_workers:
            record['client_thread_affinity'].extend(
                dict(pinned, client='capacity')
                for pinned in runtime.pin_child_workers(capacity.pid, 8, False))
        resume(capacity)
        assert event(capacity, 'events')['event'] == 'ready'
        sidecar = launch(sc, 'sidecar')
        assert event(sidecar, 'sidecar_events')['event'] == 'ready'
        if args.pin_client_workers:
            record['client_thread_affinity'].extend(
                dict(pinned, client='sidecar')
                for pinned in runtime.pin_child_workers(sidecar.pid, 8, False))
        verify_client_affinity()
        record['ready_metrics'] = metrics()
        inspection_sessions = 0 if args.http_page_size is not None else 1 + args.inspectors
        assert record['ready_metrics']['war_connections'] == args.users + args.readers + 1 + inspection_sessions + 2 * args.pairs
        record['affinity'] = [{'name': (task / 'comm').read_text().strip(), 'cpus': sorted(os.sched_getaffinity(int(task.name)))}
                              for task in Path(f'/proc/{server.pid}/task').iterdir()
                              if (task / 'comm').read_text().startswith(('war-io-', 'war-cpp-'))]
        assert len(record['affinity']) == 8 and all(len(t['cpus']) == 1 for t in record['affinity'])
        if args.profile:
            log = open(str(prefix) + '-perf.log', 'w'); files.append(log)
            target = {'server': server, 'capacity': capacity, 'sidecar': sidecar}[args.profile_target]
            command = ['perf', 'record', '-e', 'cycles:u', '-F', '499', '--call-graph', 'dwarf,8192',
                       '-p', str(target.pid), '-o', str(prefix) + '.perf.data']
            record['profile'] = {'target': args.profile_target, 'pid': target.pid, 'command': command,
                                 'scope': 'Diagnostic user-cycle samples; exclude from ordinary timing comparisons'}
            perf = subprocess.Popen(command, stdout=log, stderr=log)
            time.sleep(.2)
            code = perf.poll()
            if code is not None:
                record['profile']['exit_code'] = code
                raise RuntimeError(f'profiler exited before measurement (status {code}); see {prefix}-perf.log')
        record['before'] = snapshot(server.pid)
        record['client_before'] = {'capacity': snapshot(capacity.pid), 'sidecar': snapshot(sidecar.pid)}
        began = time.monotonic(); resume(sidecar); resume(capacity)
        while not select.select([capacity.stdout], [], [], .25)[0]:
            assert time.monotonic() - began < args.seconds + 120
            verify_client_affinity()
            start = time.monotonic(); current = metrics()
            record['monitoring'].append({'seconds': start - began, 'response_ms': (time.monotonic() - start) * 1000,
                                         'process': snapshot(server.pid, include_fds=False), 'metrics': current})
            assert current['war_connections'] >= args.users
            assert current['war_queued_bytes'] <= 256 * 1024**2
            if args.kind == 'rust':
                assert current['war_read_buffer_reserved_bytes'] <= 256 * 1024**2
                assert current['war_routing_copied_bytes_total'] == 0
        assert event(capacity, 'events')['event'] == 'measured'
        verify_client_affinity()
        record['after'] = snapshot(server.pid)
        record['client_after'] = {'capacity': snapshot(capacity.pid), 'sidecar': snapshot(sidecar.pid)}
        record['elapsed_seconds'] = time.monotonic() - began
        record['server_cpu_seconds'] = record['after']['cpu_seconds'] - record['before']['cpu_seconds']
        if perf:
            code = perf.poll()
            if code is not None:
                record['profile']['exit_code'] = code
                raise RuntimeError(f'profiler exited before controller stop (status {code}); see {prefix}-perf.log')
            perf.send_signal(signal.SIGINT)
            code = perf.wait(timeout=10)
            record['profile']['exit_code'] = code
            perf = None
            if code not in (0, 130, -signal.SIGINT):
                raise RuntimeError(f'profiler stop failed (status {code}); see {prefix}-perf.log')
            data_path = Path(str(prefix) + '.perf.data')
            if not data_path.is_file() or data_path.stat().st_size == 0:
                raise RuntimeError(f'profiler produced no data: {data_path}')
            record['profile']['data_bytes'] = data_path.stat().st_size
            record['profile']['data_sha256'] = digest(data_path)
        assert event(sidecar, 'sidecar_events')['event'] == 'measured'
        details = record['sidecar_events'][-1]['details']
        fanout = details['fanout']
        record['fanout_window_timing'] = fanout_timing_summary(details.get('fanout_controller_timing'))
        record['fanout_windows'] = {'completed': fanout['verified'] // (args.readers * args.window),
                                    'offered': args.seconds * args.fanout_windows_per_second if args.fanout_windows_per_second else None}
        offered = record['fanout_windows']['offered']
        record['fanout_offered_load_met'] = None if offered is None else record['fanout_windows']['completed'] == offered
        resume(sidecar)
        assert event(sidecar, 'sidecar_events')['event'] == 'done'; resume(sidecar)
        assert sidecar.wait(timeout=10) == 0
        assert metrics()['war_connections'] == args.users
        resume(capacity)
        assert event(capacity, 'events')['event'] == 'postcheck'; resume(capacity)
        assert event(capacity, 'events')['event'] == 'done'; resume(capacity)
        assert capacity.wait(timeout=10) == 0
        for _ in range(300):
            final = metrics()
            if final['war_connections'] == 0 and snapshot(server.pid)['fds'] == record['baseline']['fds']:
                break
            time.sleep(.01)
        record['final_metrics'] = final
        zero_keys = ['war_connections', 'war_queued_bytes', 'war_variable_bytes']
        zero_keys += ['war_sessions', 'war_output_queue_capacity_bytes', 'war_read_buffer_reserved_bytes', 'war_routing_copied_bytes_total'] if args.kind == 'rust' else ['war_users', 'war_retained_output_bytes']
        for key in zero_keys:
            assert final[key] == 0, (key, final[key])
        assert final['war_channels'] == 1
        left = record['events'][-1]['counts']
        right = record['sidecar_events'][-1]['details']['counts']
        counter_pairs = [('war_accepted_total', 'joins'), ('war_joined_total', 'joins'),
                              ('war_disconnected_total', 'normal_closes'),
                              ('war_incoming_messages_total', 'sent_messages'), ('war_incoming_bytes_total', 'sent_bytes'),
                              ('war_deliveries_total', 'deliveries'), ('war_delivered_bytes_total', 'delivered_bytes'),
                              ('war_routing_reused_bytes_total', 'value_bytes')]
        if args.kind == 'cpp':
            counter_pairs = [('war_sent_messages_total', 'sent_messages'), ('war_deliveries_total', 'deliveries'), ('war_disconnected_total', 'normal_closes')]
        for metric, field in counter_pairs:
            assert final[metric] == left[field] + right[field], (metric, final[metric], left[field], right[field])
        assert snapshot(server.pid)['fds'] == record['baseline']['fds']
        for key, value in final.items():
            if any(word in key for word in ('errors_total', 'rejected_total', 'slow_consumers_total', 'timeouts_total', 'aborted_writes_total', 'close_failures_total', 'transport_disconnects_total', 'global_pressure_total')):
                assert value == 0, (key, value)
        with open(str(prefix) + '.csv') as source:
            rows = list(csv.DictReader(source))
        assert len(rows) == args.users and len({row['id'] for row in rows}) == args.users
        assert all(int(row['samples']) == args.seconds * 4 for row in rows)
        # Short screens can rank users without establishing a per-user tail SLO.
        record['latency_sampling'] = {
            'users': len(rows),
            'observations_per_user': args.seconds * 4,
            'total_private_observations': sum(int(row['samples']) for row in rows),
            'all_user_p99_equal_observed_max': all(
                float(row['p99_ms']) == float(row['max_ms']) for row in rows),
            'scope': 'Finite per-user observations; not a latency deadline or long-term p99 guarantee',
        }
        record['csv_sha256'] = digest(str(prefix) + '.csv')
        record['latency'] = {}
        for column in ('p99_ms', 'max_ms', 'schedule_p99_ms', 'schedule_max_ms'):
            values = sorted(float(row[column]) for row in rows)
            record['latency'][column] = {name: values[(len(values) - 1) * percentile // 100]
                                        for name, percentile in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]}
        timeline_path = str(prefix) + '.windows.json'
        timeline = None
        record['generator_lag'] = {'available': False, 'reason': 'Client did not emit a latency timeline'}
        if Path(timeline_path).is_file():
            timeline = json.loads(Path(timeline_path).read_text())
            assert len(timeline['windows']) == args.seconds
            assert all(w[clock]['samples'] == args.users * 4 for w in timeline['windows']
                       for clock in ['pre_send_delay', 'send_to_receive', 'intended_to_receive'])
            record['generator_lag'] = generator_lag(timeline, int(cc[cc.index('--messages-per-second') + 1]))
            record['latency_windows_sha256'] = digest(timeline_path)
        if args.latency_stages:
            assert timeline is not None, 'Stage diagnostic requires a latency timeline'
            assert timeline['stage_order'] == ['wake_delay', 'prepare', 'room_send_delay', 'socket_send', 'after_send']
            assert len(timeline['windows']) == args.seconds
            identities = {int(row['id']) for row in rows}
            for second, window in enumerate(timeline['windows']):
                assert window['intended_second'] == second
                assert set(window['client_stages']) == set(timeline['stage_order'])
                assert all(stage['samples'] == args.users * 4 for stage in window['client_stages'].values())
                for label, probe in window['worst_stage_probes'].items():
                    stages = probe['stages_ns']
                    assert len(stages) == 5 and all(value >= 0 for value in stages)
                    assert sum(stages) == probe['intended_ns']
                    assert probe['id'] in identities and (probe['sequence'] - 1) // 4 == second
                    observed = {'intended_to_receive': probe['intended_ns'],
                                'pre_send_delay': sum(stages[:3]), 'after_send': stages[4]}[label]
                    expected = (window['client_stages']['after_send'] if label == 'after_send' else window[label])['max_ms']
                    assert abs(observed / 1e6 - expected) < 1e-9
            record['latency_stages_windows_sha256'] = digest(timeline_path)
        if args.exact_fanout_latency:
            fanout_path = str(prefix) + '-fanout.csv'
            with open(fanout_path) as source:
                audience = list(csv.DictReader(source))
            assert len(audience) == args.readers and len({r['id'] for r in audience}) == args.readers
            assert sum(int(r['samples']) for r in audience) == fanout['verified']
            assert all(int(r['samples']) == fanout['verified'] // args.readers for r in audience)
            assert all(int(r['schedule_p99_ns']) >= int(r['p99_ns']) and
                       int(r['schedule_max_ns']) >= int(r['max_ns']) for r in audience)
            record['fanout_csv_sha256'] = digest(fanout_path)
            record['fanout_exact_ms'] = {}
            for column in ('p99_ns', 'max_ns', 'schedule_p99_ns', 'schedule_max_ns'):
                values = sorted(int(r[column]) / 1e6 for r in audience)
                record['fanout_exact_ms'][column.removesuffix('_ns') + '_ms'] = {
                    name: values[(len(values) - 1) * q // 100]
                    for name, q in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]}
        record['correctness_passed'] = True
    except Exception as error:
        record['error'] = repr(error)
        raise
    finally:
        if perf and perf.poll() is None:
            perf.send_signal(signal.SIGINT); perf.wait(timeout=10)
        for proc in reversed(processes):
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    proc.kill(); proc.wait()
        record['exit_codes'] = [proc.returncode for proc in processes]
        record['correctness_passed'] = record['correctness_passed'] and all(code == 0 for code in record['exit_codes'])
        record['passed'] = record['correctness_passed'] and record.get('fanout_offered_load_met') is not False
        for handle in files:
            handle.close()
        args.output.write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({'passed': record['passed'], 'correctness_passed': record['correctness_passed'], 'fanout_windows': record.get('fanout_windows'), 'fanout_offered_load_met': record.get('fanout_offered_load_met'), 'generator_exceeded_one_period': record.get('generator_lag', {}).get('exceeded_one_period'), 'cpu': record.get('server_cpu_seconds'), 'latency': record.get('latency'), 'fanout_exact_ms': record.get('fanout_exact_ms')}))
    if not record['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
