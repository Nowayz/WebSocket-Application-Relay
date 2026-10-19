"""Finite native shutdown check, optionally warming a large room's broadcast view.

Every connection must receive Close 1001 and reach TCP EOF. The service flushes
Close and retires transports without waiting for peer acknowledgments.
Elapsed time includes the serial client drain; it is not server-only latency.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('shutdown_peers', ROOT / 'scripts/departure-fairness.py')
peers_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(peers_module)
wire = peers_module.wire


def run(args):
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    while admin == port:
        admin = wire.runtime.free_port()
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
               '--admin-bind', f'127.0.0.1:{admin}', '--workers', str(args.workers),
               '--max-connections', str(args.users), '--max-channel-members', str(args.users),
               '--ping-interval-secs', '0', '--idle-timeout-secs', '0',
               '--messages-per-second', '0', '--bytes-per-second', '0',
               '--shutdown-timeout-secs', str(args.grace_secs),
               '--tcp-send-buffer-bytes', '16384', '--tcp-receive-buffer-bytes', '65536']
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command + ['--pin-workers']
    result = {
        'passed': False, 'server_forced_kill': False, 'command': command,
        'users': args.users, 'prime_broadcast': args.prime_broadcast,
        'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'helper_sha256': {str(Path(p).relative_to(ROOT)): hashlib.sha256(Path(p).read_bytes()).hexdigest()
                          for p in [peers_module.__file__, wire.__file__, wire.runtime.__file__]},
        'client_affinity': sorted(os.sched_getaffinity(0)),
        'shutdown_closes': 0, 'tcp_eofs': 0,
    }
    peers = []
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
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
                raise TimeoutError('server readiness deadline')
            started = time.monotonic()
            for index in range(args.users):
                if time.monotonic() - started > 60:
                    raise TimeoutError('60-second population setup budget')
                peers.append(peers_module.Peer(port, b'shutdown-room', index))
            assert len({p.id for p in peers}) == args.users
            result['setup_seconds'] = time.monotonic() - started
            if args.prime_broadcast:
                source = peers[0]
                value = b'\x48prime123'
                source.socket.sendall(wire.frame(0x82, b'\x82\x1b' + b'\xff' * 8 + value))
                for peer in peers[1:]:
                    opcode, body = peer.receive()
                    assert opcode == 2 and body[0] == 0x82
                    sender, end = wire.uint(body, 1)
                    assert sender == source.id and body[end:] == value
                # This source cannot decode its next message until its bounded
                # multicast completes. The echo proves the publisher resumed.
                source.socket.sendall(wire.frame(0x82, source.packet(b'\xf5')))
                source.verify(b'\xf5')
                result['broadcast_receivers_verified'] = args.users - 1
            for _ in range(200):
                metrics = wire.runtime.metrics(admin)
                if (int(metrics['war_connections']) == args.users
                        and int(metrics['war_sessions']) == args.users
                        and int(metrics['war_queued_bytes']) == 0):
                    break
                time.sleep(.01)
            else:
                raise AssertionError('population did not reach quiescence')
            assert int(metrics['war_routing_copied_bytes_total']) == 0
            for key in wire.ZERO_ERRORS:
                assert int(metrics[key]) == 0, (key, metrics[key])
            result['before'] = metrics
            result['process_before'] = wire.runtime.snapshot(server.pid)
            started_ns = time.monotonic_ns()
            result['shutdown_started_ns'] = started_ns
            server.terminate()
            for peer in peers:
                opcode, body = peer.receive()
                assert opcode == 8 and len(body) >= 2 and int.from_bytes(body[:2], 'big') == 1001, (opcode, body.hex())
                result['shutdown_closes'] += 1
                result.setdefault('first_close_ms', (time.monotonic_ns() - started_ns) / 1e6)
                assert peer.socket.recv(1) == b'', 'missing transport EOF'
                result['tcp_eofs'] += 1
                result.setdefault('first_eof_ms', (time.monotonic_ns() - started_ns) / 1e6)
                peer.socket.close()
            result['last_eof_ms'] = (time.monotonic_ns() - started_ns) / 1e6
            assert server.wait(timeout=args.grace_secs + 2) == 0
            result['shutdown_elapsed_ms'] = (time.monotonic_ns() - started_ns) / 1e6
            result['passed'] = True
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            for peer in peers:
                peer.socket.close()
            if server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=args.grace_secs + 2)
                except subprocess.TimeoutExpired:
                    result['server_forced_kill'] = True
                    server.kill()
                    server.wait(timeout=3)
            result['server_exit'] = server.returncode
            if result['server_forced_kill'] or server.returncode != 0:
                result['passed'] = False
            log.seek(0)
            result['server_log'] = log.read().decode(errors='replace')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--users', type=int, default=2048)
    parser.add_argument('--workers', type=int, default=8)
    parser.add_argument('--server-cpus', default='')
    parser.add_argument('--prime-broadcast', action='store_true')
    parser.add_argument('--grace-secs', type=int, default=10)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.users < 1 or args.workers < 1 or args.grace_secs < 1:
        parser.error('users, workers and shutdown grace must be positive')
    with args.output.open('x') as output:
        result = run(args)
        json.dump(result, output, indent=2)
        output.write('\n')
    print(json.dumps({k: result.get(k) for k in [
        'passed', 'users', 'prime_broadcast', 'shutdown_elapsed_ms', 'first_close_ms',
        'first_eof_ms', 'shutdown_closes', 'tcp_eofs', 'error']}))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
