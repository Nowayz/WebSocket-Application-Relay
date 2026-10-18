#!/usr/bin/env python3
"""Gated population fairness with checked room multicast, churn and CBOR inspection.

Two clients use disjoint CPU sets and disjoint rooms. Capacity is held connected
until sidecar inspection and cleanup finish, preserving its exact expected census.
All server timing excludes setup, CSV output and teardown. Never time in parallel.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def snapshot(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    status = Path(f'/proc/{pid}/status').read_text().splitlines()
    return {'cpu_seconds': (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'),
            'rss_kib': int(next(line.split()[1] for line in status if line.startswith('VmRSS:'))),
            'fds': len(list(Path(f'/proc/{pid}/fd').iterdir()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--kind', choices=['rust', 'cpp'], default='rust')
    parser.add_argument('--sidecar', type=Path, default=ROOT / 'target/release/examples/inspection')
    parser.add_argument('--capacity-client', type=Path, default=ROOT / 'artifacts/candidates/latency-timeline-client/capacity')
    parser.add_argument('--users', type=int, default=100000)
    parser.add_argument('--seconds', type=int, default=30)
    parser.add_argument('--readers', type=int, default=8192)
    parser.add_argument('--scope', choices=['room', 'observers'], default='observers')
    parser.add_argument('--window', type=int, default=4)
    parser.add_argument('--inspectors', type=int, default=2)
    parser.add_argument('--pairs', type=int, default=1)
    parser.add_argument('--churn-cycles-per-second', type=int, default=0)
    parser.add_argument('--fanout-windows-per-second', type=int, default=0)
    parser.add_argument('--fanout-churn-every', type=int, default=0)
    parser.add_argument('--exact-fanout-latency', action='store_true')
    parser.add_argument('--profile', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    assert args.users % 16 == 0 and args.seconds >= 2
    prefix = args.output.with_suffix('')
    assert not args.output.exists() and not Path(str(prefix) + '.csv').exists()
    client_path = args.capacity_client.resolve()
    port, admin = free_port(), free_port()
    while admin == port:
        admin = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env.update(WAR_ADMIN_TOKEN='mixed-capacity-fixture', WAR_AUTH_PASSWORD='inspection-benchmark')
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
    sc = ['taskset', '-c', '12-15,28-31', str(args.sidecar.resolve()), '--url', f'ws://127.0.0.1:{port}',
          '--protocol-only', '--mode', 'cbor', '--rooms', '0', '--pairs', str(args.pairs), '--seconds', str(args.seconds),
          '--churn-workers', '8', '--churn-cycles-per-second', str(args.churn_cycles_per_second), '--inspectors', str(args.inspectors), '--client-workers', '8',
          '--fanout-readers', str(args.readers), '--fanout-window', str(args.window), '--fanout-global-observers',
          '--external-capacity-rooms', str(args.users // 16), '--fanout-close-window', '32', '--fanout-windows-per-second', str(args.fanout_windows_per_second)]
    if args.fanout_churn_every:
        assert args.scope == 'room' and args.inspectors == 0
        sc[sc.index('--mode') + 1] = 'none'
        sc += ['--fanout-churn-every', str(args.fanout_churn_every)]
    if args.exact_fanout_latency:
        sc += ['--fanout-latency-output', str(prefix) + '-fanout.csv']
    if args.scope == 'room':
        sc.remove('--fanout-global-observers')
    record = {'passed': False, 'args': vars(args).copy(), 'server_command': cmd, 'client_command': cc,
              'sidecar_command': sc, 'sha256': {'server': digest(args.server), 'capacity_client': digest(client_path),
              'sidecar': digest(args.sidecar), 'runner': digest(__file__)}, 'events': [], 'sidecar_events': [], 'monitoring': []}
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
        request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization': 'Bearer mixed-capacity-fixture'})
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
        assert event(capacity, 'events')['event'] == 'connected'; resume(capacity)
        assert event(capacity, 'events')['event'] == 'ready'
        sidecar = launch(sc, 'sidecar')
        assert event(sidecar, 'sidecar_events')['event'] == 'ready'
        record['ready_metrics'] = metrics()
        assert record['ready_metrics']['war_connections'] == args.users + args.readers + 2 + args.inspectors + 2 * args.pairs
        record['affinity'] = [{'name': (task / 'comm').read_text().strip(), 'cpus': sorted(os.sched_getaffinity(int(task.name)))}
                              for task in Path(f'/proc/{server.pid}/task').iterdir()
                              if (task / 'comm').read_text().startswith(('war-io-', 'war-cpp-'))]
        assert len(record['affinity']) == 8 and all(len(t['cpus']) == 1 for t in record['affinity'])
        if args.profile:
            log = open(str(prefix) + '-perf.log', 'w'); files.append(log)
            perf = subprocess.Popen(['perf', 'record', '-e', 'cycles:u', '-F', '499', '--call-graph', 'dwarf,8192',
                                     '-p', str(server.pid), '-o', str(prefix) + '.perf.data'], stdout=log, stderr=log)
            time.sleep(.2)
        record['before'] = snapshot(server.pid)
        record['client_before'] = {'capacity': snapshot(capacity.pid), 'sidecar': snapshot(sidecar.pid)}
        began = time.monotonic(); resume(sidecar); resume(capacity)
        while not select.select([capacity.stdout], [], [], .25)[0]:
            assert time.monotonic() - began < args.seconds + 120
            start = time.monotonic(); current = metrics()
            record['monitoring'].append({'seconds': start - began, 'response_ms': (time.monotonic() - start) * 1000,
                                         'process': snapshot(server.pid), 'metrics': current})
            assert current['war_connections'] >= args.users
            assert current['war_queued_bytes'] <= 256 * 1024**2
            if args.kind == 'rust':
                assert current['war_read_buffer_reserved_bytes'] <= 256 * 1024**2
                assert current['war_routing_copied_bytes_total'] == 0
        assert event(capacity, 'events')['event'] == 'measured'
        record['after'] = snapshot(server.pid)
        record['client_after'] = {'capacity': snapshot(capacity.pid), 'sidecar': snapshot(sidecar.pid)}
        record['elapsed_seconds'] = time.monotonic() - began
        record['server_cpu_seconds'] = record['after']['cpu_seconds'] - record['before']['cpu_seconds']
        if perf:
            perf.send_signal(signal.SIGINT); assert perf.wait(timeout=10) in (0, 130, -signal.SIGINT); perf = None
        assert event(sidecar, 'sidecar_events')['event'] == 'measured'
        fanout = record['sidecar_events'][-1]['details']['fanout']
        record['fanout_windows'] = {'completed': fanout['verified'] // (args.readers * args.window),
                                    'offered': args.seconds * args.fanout_windows_per_second if args.fanout_windows_per_second else None}
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
        record['csv_sha256'] = digest(str(prefix) + '.csv')
        record['latency'] = {}
        for column in ('p99_ms', 'max_ms', 'schedule_p99_ms', 'schedule_max_ms'):
            values = sorted(float(row[column]) for row in rows)
            record['latency'][column] = {name: values[(len(values) - 1) * percentile // 100]
                                        for name, percentile in [('median', 50), ('p95', 95), ('p99', 99), ('max', 100)]}
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
        record['passed'] = True
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
        record['passed'] = record['passed'] and all(code == 0 for code in record['exit_codes'])
        for handle in files:
            handle.close()
        args.output.write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({'passed': record['passed'], 'cpu': record.get('server_cpu_seconds'), 'latency': record.get('latency'), 'fanout_exact_ms': record.get('fanout_exact_ms')}))
    if not record['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
