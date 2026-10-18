#!/usr/bin/env python3
"""Gated selected-Rust versus owner-fanout per-user comparison; server CPU excludes setup and CSV output."""
import argparse, csv, hashlib, json, os, pathlib, selectors, signal, socket, subprocess, time, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument('--users', type=int, default=100000)
p.add_argument('--seconds', type=int, default=30)
p.add_argument('--workers', type=int, default=8)
p.add_argument('--rate', type=int, default=4)
p.add_argument('--repeats', type=int, default=2)
p.add_argument('--output', type=pathlib.Path, required=True)
a = p.parse_args()

def free():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def digest(f):
    return hashlib.sha256(pathlib.Path(f).read_bytes()).hexdigest()

def snap(pid):
    fields = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    status = pathlib.Path(f'/proc/{pid}/status').read_text().splitlines()
    result = {'cpu_seconds': (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'), 'rss_kib': int(next((s.split()[1] for s in status if s.startswith('VmRSS:')))), 'fd_count': len(list(pathlib.Path(f'/proc/{pid}/fd').iterdir()))}
    rollup = {line.split(':')[0]: int(line.split()[1]) for line in pathlib.Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines() if line.startswith(('Pss:', 'Private_Clean:', 'Private_Dirty:'))}
    result['pss_kib'] = rollup['Pss']
    result['private_kib'] = rollup['Private_Clean'] + rollup['Private_Dirty']
    return result

def metrics(port):
    req = urllib.request.Request(f'http://127.0.0.1:{port}/metrics', headers={'Authorization': 'Bearer cpp-capacity-fixture'})
    with urllib.request.urlopen(req, timeout=3) as r:
        return {k: int(v) for k, v in (s.split() for s in r.read().decode().splitlines() if s.startswith('war_'))}
servers = {'rust': ROOT / 'artifacts/candidates/observer-quantum/war-relay', 'rust-owner': ROOT / 'artifacts/candidates/rust-owner-fanout-isolated/war-relay'}
client = ROOT / 'artifacts/candidates/latency-timeline-client/capacity'
record = {'scope': 'Same frozen gated capacity client and native CBOR.100%users,16memberrooms,1KiB,4probes/user/second by default,heartbeat disabled in both,16KiB send/64KiB receive requested socket buffers,8pinnedserver CPUs0..7/client8..15. Measured server CPU between ready/measured gates; CSV sorting/file output excluded. Exact per-user send-to-receive and intended-to-receive percentiles include client/room scheduling. Fresh sequential rotated servers. Both are selected Rust plus/minus owner-queued zero-copy fanout; all private probe settings and policies identical.', 'server_sha256': {k: digest(v) for k, v in servers.items()}, 'client_sha256': digest(client), 'runs': [], 'passed': False, 'runner_sha256': hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()}
for rep in range(a.repeats):
    for kind in ['rust', 'rust-owner'] if rep % 2 == 0 else ['rust-owner', 'rust']:
        prefix = a.output.with_suffix('').as_posix() + f'-{kind}-{rep + 1}'
        assert not pathlib.Path(prefix + '.csv').exists()
        port, admin = (free(), free())
        row = {'kind': kind, 'repeat': rep + 1, 'passed': False, 'gates': []}
        cmd = ['taskset', '-c', f'0-{a.workers - 1}', str(servers[kind]), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', str(a.workers), '--pin-workers', '--max-connections', str(a.users + 1000), '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536']
        if kind.startswith('rust'):
            cmd += ['--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0', '--idle-timeout-secs', '0']
        env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
        env['WAR_ADMIN_TOKEN'] = 'cpp-capacity-fixture'
        row['server_command'] = cmd
        print(json.dumps({'start': kind, 'repeat': rep + 1}), flush=True)
        with open(prefix + '-server.log', 'w') as log:
            server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
            load = None
            try:
                for _ in range(300):
                    if server.poll() is not None:
                        raise RuntimeError('server exited')
                    try:
                        metrics(admin)
                        break
                    except OSError:
                        time.sleep(0.01)
                else:
                    raise TimeoutError('startup')
                row['baseline'] = snap(server.pid)
                cc = ['taskset', '-c', '8-15', str(client), '--url', f'ws://127.0.0.1:{port}', '--users', str(a.users), '--seconds', str(a.seconds), '--messages-per-second', str(a.rate), '--client-workers', '8', '--user-latency-output', prefix + '.csv']
                row['client_command'] = cc
                with open(prefix + '-client-error.log', 'w') as err:
                    load = subprocess.Popen(cc, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err, text=True, bufsize=1)
                    selector = selectors.DefaultSelector()
                    selector.register(load.stdout, selectors.EVENT_READ)
                    deadline = time.monotonic() + 240
                    with open(prefix + '-client.log', 'w') as output:
                        while True:
                            if not selector.select(timeout=1):
                                assert time.monotonic() < deadline, 'client deadline'
                                if load.poll() is not None:
                                    break
                                continue
                            line = load.stdout.readline()
                            if not line:
                                break
                            output.write(line)
                            output.flush()
                            event = json.loads(line)
                            phase = event.get('event')
                            row['gates'].append(event)
                            if phase in ['connected', 'ready', 'measured', 'postcheck', 'done']:
                                event['server_snapshot'] = snap(server.pid)
                                event['server_metrics'] = metrics(admin)
                                if phase == 'ready':
                                    assert event['server_metrics']['war_connections'] == a.users, event
                                    row['thread_affinity'] = []
                                    for task in pathlib.Path(f'/proc/{server.pid}/task').iterdir():
                                        name = (task / 'comm').read_text().strip()
                                        if name.startswith('war-io-') or name.startswith('war-cpp-'):
                                            row['thread_affinity'].append({'tid': int(task.name), 'name': name, 'cpus': sorted(os.sched_getaffinity(int(task.name)))})
                                    assert len(row['thread_affinity']) == a.workers and all((len(t['cpus']) == 1 for t in row['thread_affinity']))
                                    row['measured_before'] = snap(server.pid)
                                if phase == 'measured':
                                    after = snap(server.pid)
                                    row['server_cpu_seconds'] = after['cpu_seconds'] - row['measured_before']['cpu_seconds']
                                    row['population_rss_kib'] = row['measured_before']['rss_kib']
                                load.stdin.write('continue\n')
                                load.stdin.flush()
                    row['client_exit_code'] = load.wait(timeout=15)
                    assert row['client_exit_code'] == 0, pathlib.Path(prefix + '-client-error.log').read_text()
                for _ in range(300):
                    final = metrics(admin)
                    if final['war_connections'] == 0 and final['war_queued_bytes'] == 0:
                        break
                    time.sleep(0.01)
                for key in ['war_connections', 'war_queued_bytes', 'war_variable_bytes']:
                    assert final[key] == 0, final
                assert final.get('war_users', final.get('war_sessions')) == 0, final
                assert final['war_channels'] == 1, final
                assert not final.get('war_slow_consumers_total', 0), final
                row['final_metrics'] = final
                row['after'] = snap(server.pid)
                assert row['after']['fd_count'] == row['baseline']['fd_count'], row
                rows = list(csv.DictReader(open(prefix + '.csv')))
                assert len(rows) == a.users and len({r['id'] for r in rows}) == a.users and all((int(r['samples']) == a.seconds * a.rate for r in rows))
                row['users'] = a.users
                row['samples_each'] = a.seconds * a.rate
                row['csv'] = prefix + '.csv'
                row['csv_sha256'] = digest(prefix + '.csv')
                for column in ['p99_ms', 'max_ms', 'schedule_p99_ms', 'schedule_max_ms']:
                    values = sorted((float(r[column]) for r in rows))
                    row[column] = {'median': values[(len(values) - 1) // 2], 'p95': values[(len(values) - 1) * 95 // 100], 'p99': values[(len(values) - 1) * 99 // 100], 'max': values[-1]}
                row['passed'] = True
            except Exception as error:
                row['error'] = str(error)
            finally:
                if load and load.poll() is None:
                    load.kill()
                    load.wait()
                server.terminate()
                try:
                    row['server_exit_code'] = server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait()
                    row['server_exit_code'] = 'killed'
                    row['passed'] = False
                row['passed'] &= row['server_exit_code'] == 0
        record['runs'].append(row)
        a.output.write_text(json.dumps(record, indent=2) + '\n')
        print(json.dumps({k: v for k, v in row.items() if k not in ['gates', 'final_metrics', 'server_command', 'client_command']}), flush=True)
        if not row['passed']:
            raise SystemExit(1)
record['passed'] = True
a.output.write_text(json.dumps(record, indent=2) + '\n')
