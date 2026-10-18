#!/usr/bin/env python3
"""Compare private forwarding with /proc-verified local or cross-worker pairs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
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
    return {'cpu_seconds': (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK'),
            'fd_count': len(list(Path(f'/proc/{pid}/fd').iterdir()))}


def metrics(port):
    request = urllib.request.Request(f'http://127.0.0.1:{port}/metrics',
                                    headers={'Authorization': 'Bearer placement-fixture'})
    with urllib.request.urlopen(request, timeout=3) as response:
        return {key: int(value) for key, value in
                (line.split() for line in response.read().decode().splitlines() if line.startswith('war_'))}


def ownership(pid, port, peers):
    """Map each live client port through TCP inode -> server fd -> epoll fd."""
    tcp = {}
    for line in Path(f'/proc/{pid}/net/tcp').read_text().splitlines()[1:]:
        fields = line.split()
        if int(fields[1].split(':')[1], 16) == port and fields[3] == '01':
            tcp[fields[9]] = int(fields[2].split(':')[1], 16)
    sockets, epolls = {}, {}
    for fd in Path(f'/proc/{pid}/fd').iterdir():
        try:
            target = os.readlink(fd)
        except FileNotFoundError:
            continue
        if target.startswith('socket:[') and target[8:-1] in tcp:
            sockets[int(fd.name)] = tcp[target[8:-1]]
        elif target == 'anon_inode:[eventpoll]':
            epolls[int(fd.name)] = {int(value) for value in
                re.findall(r'^tfd:\s*(\d+)', Path(f'/proc/{pid}/fdinfo/{fd.name}').read_text(), re.M)}
    mapped = {}
    for fd, client_port in sockets.items():
        owners = [epoll for epoll, watched in epolls.items() if fd in watched]
        assert len(owners) == 1, (fd, owners)
        assert client_port not in mapped, client_port
        mapped[client_port] = {'server_fd': fd, 'epoll_fd': owners[0]}
    assert len(mapped) == len(peers), (len(mapped), len(peers))
    return [{**peer, **mapped[peer['port']]} for peer in peers]


def pairing(peers, workers, pairs, mode):
    groups = {}
    for peer in peers:
        groups.setdefault(peer['epoll_fd'], []).append(peer['index'])
    assert len(groups) == workers, {key: len(value) for key, value in groups.items()}
    groups = list(groups.values())
    each = pairs // workers
    assert all(len(group) >= each * 2 for group in groups), [len(group) for group in groups]
    plan = []
    for index, group in enumerate(groups):
        for n in range(each):
            plan.append([group[n * 2], group[n * 2 + 1]] if mode == 'local'
                        else [group[n], groups[(index + 1) % workers][each + n]])
    assert len({index for pair in plan for index in pair}) == 2 * pairs
    for a, b in plan:
        assert (peers[a]['epoll_fd'] == peers[b]['epoll_fd']) == (mode == 'local')
    return plan


def run(args, label, executable, mode, repeat):
    prefix = str(args.output.with_suffix('')) + f'-{mode}-{label}-{repeat}'
    port, admin = free_port(), free_port()
    env = {key: value for key, value in os.environ.items() if not key.startswith('WAR_')}
    env['WAR_ADMIN_TOKEN'] = 'placement-fixture'
    command = ['taskset', '-c', f'0-{args.workers - 1}', str(executable.resolve()),
               '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
               '--workers', str(args.workers), '--pin-workers']
    client_command = ['taskset', '-c', '8-15', str(args.client.resolve()), '--url',
                      f'ws://127.0.0.1:{port}', '--pool', str(args.pool), '--seconds',
                      str(args.seconds), '--window', str(args.window),
                      '--payload-bytes', str(args.payload_bytes)]
    result = {'kind': label, 'mode': mode, 'repeat': repeat, 'server_command': command,
              'client_command': client_command, 'passed': False, 'gates': []}
    load = None
    with open(prefix + '-server.log', 'w') as server_log:
        server = subprocess.Popen(command, env=env, stdout=server_log, stderr=server_log)
        try:
            for _ in range(300):
                assert server.poll() is None, 'server startup failed'
                try:
                    metrics(admin)
                    break
                except OSError:
                    time.sleep(.01)
            else:
                raise TimeoutError('server startup')
            result['baseline'] = snapshot(server.pid)
            with open(prefix + '-client-error.log', 'w') as error_log, open(prefix + '-client.log', 'w') as client_log:
                load = subprocess.Popen(client_command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=error_log, text=True, bufsize=1)
                selector = selectors.DefaultSelector()
                selector.register(load.stdout, selectors.EVENT_READ)
                deadline = time.monotonic() + args.seconds + 90
                while True:
                    if not selector.select(timeout=1):
                        assert time.monotonic() < deadline, 'client deadline'
                        continue
                    line = load.stdout.readline()
                    if not line:
                        break
                    client_log.write(line)
                    client_log.flush()
                    event = json.loads(line)
                    result['gates'].append(event)
                    phase = event['event']
                    if phase == 'connected':
                        peers = ownership(server.pid, port, event['peers'])
                        assert all(peer['id'] >= 2**32 for peer in peers), 'workload requires nine-byte IDs'
                        plan = pairing(peers, args.workers, args.pairs, mode)
                        result['ownership'] = peers
                        result['plan'] = plan
                        load.stdin.write(json.dumps(plan) + '\n')
                        load.stdin.flush()
                    elif phase == 'ready':
                        assert event['pairs'] == args.pairs
                        result['metrics_before'] = metrics(admin)
                        assert result['metrics_before']['war_connections'] == args.pool
                        result['before'] = snapshot(server.pid)
                        load.stdin.write('continue\n')
                        load.stdin.flush()
                    elif phase == 'measured':
                        result['after_measured'] = snapshot(server.pid)
                        result['metrics_measured'] = metrics(admin)
                        result['result'] = event
                        result['server_cpu_seconds'] = result['after_measured']['cpu_seconds'] - result['before']['cpu_seconds']
                        before, after = result['metrics_before'], result['metrics_measured']
                        assert after['war_deliveries_total'] - before['war_deliveries_total'] == event['verified_deliveries']
                        copied = after['war_copied_bytes_total'] - before['war_copied_bytes_total']
                        result['measured_copied_bytes'] = copied
                        assert copied == (0 if mode == 'local' else event['expected_encoded_bytes']), (mode, copied, event)
                        assert ownership(server.pid, port, event_peers := result['gates'][0]['peers']) == peers
                        load.stdin.write('continue\n')
                        load.stdin.flush()
                    elif phase == 'postcheck':
                        assert event['pairs'] == args.pairs
                        load.stdin.write('continue\n')
                        load.stdin.flush()
                    elif phase == 'done':
                        assert event['normal_close_and_eof'] == args.pool
                result['client_exit_code'] = load.wait(timeout=10)
                assert result['client_exit_code'] == 0, Path(prefix + '-client-error.log').read_text()
            assert [gate['event'] for gate in result['gates']] == ['connected', 'ready', 'measured', 'postcheck', 'done']
            for _ in range(300):
                final = metrics(admin)
                if final['war_connections'] == 0:
                    break
                time.sleep(.01)
            for key in ['war_connections', 'war_users', 'war_queued_bytes', 'war_retained_output_bytes', 'war_buffered_bytes', 'war_variable_bytes']:
                assert final[key] == 0, (key, final)
            assert final['war_channels'] == 1
            result['final_metrics'] = final
            result['after'] = snapshot(server.pid)
            assert result['after']['fd_count'] == result['baseline']['fd_count']
            result['passed'] = True
        except Exception as error:
            result['error'] = str(error)
        finally:
            if load is not None and load.poll() is None:
                load.kill()
                load.wait()
            server.terminate()
            try:
                result['server_exit_code'] = server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
                result['server_exit_code'] = 'killed'
            result['passed'] &= result['server_exit_code'] == 0
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', action='append', required=True, help='LABEL=EXECUTABLE')
    parser.add_argument('--client', type=Path, default=ROOT / 'target/release/examples/placement')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--workers', type=int, default=4)
    parser.add_argument('--pairs', type=int, default=32)
    parser.add_argument('--pool', type=int, default=256)
    parser.add_argument('--seconds', type=int, default=8)
    parser.add_argument('--window', type=int, default=16)
    parser.add_argument('--payload-bytes', type=int, default=1024)
    parser.add_argument('--repeats', type=int, default=4)
    parser.add_argument('--modes', nargs='+', choices=['local', 'remote'], default=['local', 'remote'])
    args = parser.parse_args()
    assert args.workers > 1 and args.pairs > 0 and args.pairs % args.workers == 0
    assert args.seconds > 0 and args.repeats > 0 and args.pool >= 2 * args.pairs
    assert not args.output.exists(), args.output
    servers = {}
    for specification in args.server:
        label, separator, path = specification.partition('=')
        assert separator and re.fullmatch(r'[A-Za-z0-9_-]+', label) and label not in servers
        servers[label] = Path(path)
    record = {'scope': 'Linux plaintext native CBOR. /proc TCP inode/fd/epoll ownership determines balanced disjoint pairs. All pairs local or remote; idle pool remains connected. Server CPU measured between gates; topology queries, templates, postchecks and Close/EOF excluded. Window latency includes client scheduling; not isolated relay-hop latency. Fresh sequential rotated processes, server CPUs0..3 by default, client8..15.',
              'server_sha256': {label: digest(path) for label, path in servers.items()},
              'client_sha256': digest(args.client), 'runner_sha256': digest(__file__),
              'runs': [], 'passed': False}
    for mode in args.modes:
        for repeat in range(1, args.repeats + 1):
            for label in list(servers) if repeat % 2 else list(reversed(servers)):
                print(json.dumps({'start': label, 'mode': mode, 'repeat': repeat}), flush=True)
                row = run(args, label, servers[label], mode, repeat)
                record['runs'].append(row)
                args.output.write_text(json.dumps(record, indent=2) + '\n')
                print(json.dumps({key: row[key] for key in ['kind', 'mode', 'repeat', 'passed', 'error'] if key in row}), flush=True)
                if not row['passed']:
                    raise SystemExit(1)
    record['passed'] = True
    args.output.write_text(json.dumps(record, indent=2) + '\n')


if __name__ == '__main__':
    main()
