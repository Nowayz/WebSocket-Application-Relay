#!/usr/bin/env python3
"""Checked observer fanout, private traffic, churn and CBOR inspection in competition."""
import argparse, hashlib, json, os, pathlib, signal, socket, subprocess, time, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[1]
p = argparse.ArgumentParser()
p.add_argument('--seconds', type=int, default=5)
p.add_argument('--output', type=pathlib.Path, required=True)
p.add_argument('--workers', type=int, nargs='+', default=[4])
p.add_argument('--modes', nargs='+', default=['mixed'])
p.add_argument('--repeats', type=int, default=2)
p.add_argument('--include-rust-candidate', action='store_true')
a = p.parse_args()

def free():
    with socket.socket() as s:
        s.bind(('127.0.0.1', 0))
        return s.getsockname()[1]

def snapshot(pid):
    text = pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    rss = 0
    for row in pathlib.Path(f'/proc/{pid}/status').read_text().splitlines():
        if row.startswith('VmRSS:'):
            rss = int(row.split()[1])
    return {'cpu_seconds': (int(text[11]) + int(text[12])) / os.sysconf('SC_CLK_TCK'), 'rss_kib': rss}

def digest(f):
    return hashlib.sha256(pathlib.Path(f).read_bytes()).hexdigest()
servers = {'rust': ROOT / 'artifacts/candidates/observer-quantum/war-relay', 'cpp': ROOT / 'artifacts/candidates/cpp-accounting-layout-control/war-relay-cpp', 'rust-owner': ROOT / 'artifacts/candidates/rust-owner-fanout-isolated/war-relay'}
if a.include_rust_candidate:
    servers['rust-observer-index'] = ROOT / 'artifacts/candidates/observer-index/war-relay'
client = ROOT / 'artifacts/candidates/observer-quantum-client/inspection'
record = {'scope': 'Same frozen checked native CBOR client, protocol-only setup/cleanup gates, no HTTP inspection/bridge. 512authenticated global channel observers, 1privatepair, 64idlerooms, 8churnworkers, 2CBORinspectors/10ms, 1KiB16frame fanout windows, 8clientworkers8..15. Fresh sequential rotated4worker servers pinned0..3; rates/heartbeatdisabled. CPU includes setup/teardown and every competing workload; not isolated per-message CPU. Histogram percentile upper bounds include client windowing. C++ operating-policy gaps remain.', 'server_sha256': {k: digest(v) for k, v in servers.items()}, 'client_sha256': digest(client), 'runs': [], 'passed': False, 'runner_sha256': hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()}
for workers in a.workers:
    for mode in a.modes:
        for rep in range(a.repeats):
            for kind in list(servers) if rep % 2 == 0 else list(reversed(servers)):
                port, admin = (free(), free())
                prefix = a.output.with_suffix('').as_posix() + f'-{workers}w-{mode}-{kind}-{rep + 1}'
                cmd = ['taskset', '-c', f'0-{workers - 1}' if workers > 1 else '0', str(servers[kind]), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', str(workers), '--pin-workers']
                if kind.startswith('rust'):
                    cmd += ['--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0', '--idle-timeout-secs', '0']
                env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
                env['WAR_ADMIN_TOKEN'] = 'cpp-comparison-fixture'
                env['WAR_AUTH_PASSWORD'] = 'inspection-benchmark'
                row = {'kind': kind, 'workers': workers, 'mode': mode, 'repeat': rep + 1, 'server_command': cmd, 'passed': False}
                print(json.dumps({'start': kind, 'workers': workers, 'mode': mode, 'repeat': rep + 1}), flush=True)
                with open(prefix + '-server.log', 'w') as log:
                    server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
                    try:
                        for _ in range(300):
                            if server.poll() is not None:
                                raise RuntimeError('server exited')
                            try:
                                with urllib.request.urlopen(f'http://127.0.0.1:{admin}/readyz', timeout=0.2):
                                    break
                            except OSError:
                                time.sleep(0.01)
                        else:
                            raise TimeoutError('startup')
                        threads = []
                        for task in pathlib.Path(f'/proc/{server.pid}/task').iterdir():
                            name = (task / 'comm').read_text().strip()
                            if name.startswith('war-io-') or name.startswith('war-cpp-'):
                                threads.append({'tid': int(task.name), 'name': name, 'cpus': sorted(os.sched_getaffinity(int(task.name)))})
                        assert len(threads) == workers and all((len(t['cpus']) == 1 for t in threads)), threads
                        row['thread_affinity'] = threads
                        before = snapshot(server.pid)
                        cc = ['taskset', '-c', '8-15', str(client), '--url', f'ws://127.0.0.1:{port}', '--protocol-only', '--mode', 'cbor', '--rooms', '64', '--seconds', str(a.seconds), '--inspectors', '2', '--pairs', '1', '--churn-workers', '8', '--client-workers', '8', '--fanout-readers', '512', '--fanout-global-observers', '--fanout-window', '16']
                        row['client_command'] = cc
                        result = subprocess.run(cc, input='continue\n' * 8, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=a.seconds + 60)
                        pathlib.Path(prefix + '-client.log').write_text(result.stdout + result.stderr)
                        after = snapshot(server.pid)
                        assert result.returncode == 0, result.stderr
                        events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                        row['result'] = next((e['details'] for e in events if e.get('event') == 'measured'))
                        row['server_cpu_seconds'] = after['cpu_seconds'] - before['cpu_seconds']
                        row['rss_kib'] = after['rss_kib']
                        for _ in range(200):
                            request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization': 'Bearer cpp-comparison-fixture'})
                            with urllib.request.urlopen(request, timeout=2) as response:
                                final = {k: int(v) for k, v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_'))}
                            if final['war_connections'] == 0 and final['war_queued_bytes'] == 0:
                                break
                            time.sleep(0.01)
                        assert final['war_connections'] == final['war_queued_bytes'] == final['war_variable_bytes'] == 0, final
                        assert final.get('war_users', final.get('war_sessions')) == 0 and final['war_channels'] == 1, final
                        row['final_metrics'] = final
                        row['passed'] = True
                    except Exception as e:
                        row['error'] = str(e)
                    finally:
                        server.send_signal(signal.SIGTERM)
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
                print(json.dumps(row), flush=True)
                if not row['passed']:
                    raise SystemExit(1)
record['passed'] = True
a.output.write_text(json.dumps(record, indent=2) + '\n')
