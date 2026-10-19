"""Finite native CBOR observer-revocation and independent-echo event screen.

Default teardown drains each departure before publishing the next, so a one-slot
queue check does not also require absorbing an unrelated two-departure burst.
Timings include client/kernel scheduling; they are not per-user tail guarantees.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import subprocess
import tempfile
import time
import traceback
import threading
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('departure', ROOT / 'scripts/departure-fairness.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
wire = fixture.wire

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--audience', type=int, default=1024)
    parser.add_argument('--queue-messages', type=int)
    parser.add_argument('--server-cpus', default='')
    parser.add_argument('--workers', type=int, default=8)
    parser.add_argument('--burst-teardown', action='store_true', help='Diagnostic: publish two final departures before draining recipients')
    args = parser.parse_args()
    if args.audience < 1 or args.workers < 1:
        parser.error('audience and workers must be positive')
    port, admin = (wire.runtime.free_port(), wire.runtime.free_port())
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
               '--admin-bind', f'127.0.0.1:{admin}', '--workers', str(args.workers),
               '--max-connections', str(args.audience + 3),
               '--max-channel-members', str(args.audience + 1),
               '--ping-interval-secs', '0', '--idle-timeout-secs', '0',
               '--messages-per-second', '0', '--bytes-per-second', '0',
               '--shutdown-timeout-secs', '10', '--tcp-send-buffer-bytes', '16384',
               '--tcp-receive-buffer-bytes', '65536']
    if args.server_cpus:
        command = ['taskset', '-c', args.server_cpus] + command + ['--pin-workers']
    if args.queue_messages is not None:
        command += ['--queue-messages', str(args.queue_messages)]
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env['WAR_AUTH_PASSWORD'] = 'observer-barrier'
    helpers = [Path(fixture.__file__), Path(wire.__file__), Path(wire.runtime.__file__)]
    result = {
        'passed': False, 'command': command,
        'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
        'fixture_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        'helper_sha256': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in helpers},
        'audience': args.audience, 'burst_teardown': args.burst_teardown,
        'client_affinity': sorted(os.sched_getaffinity(0)),
        'mode': 'global_observers', 'sequence': [], 'server_forced_kill': False,
    }
    peers = []
    audience = []
    probe_stop = threading.Event()
    probe_started = threading.Event()
    probe_thread = None
    probe_samples = []
    probe_errors = []

    def join(name):
        p = fixture.Peer(port, name, len(peers))
        peers.append(p)
        return p

    def encode_uint(n):
        return bytes([27]) + n.to_bytes(8, 'big')

    def request(peer, body, expected):
        peer.socket.sendall(wire.frame(0x82, body))
        assert peer.receive() == (2, expected)

    def departure(peer, source):
        opcode, body = peer.receive()
        assert opcode == 2, {'recipient': peer.id, 'opcode': opcode, 'body': body.hex()}
        notice(body, source)

    def notice(body, source):
        assert body[0] == 0x82
        sender, offset = wire.uint(body, 1)
        value, end = wire.uint(body, offset)
        assert sender == 2 ** 64 - 1 and value == source and (end == len(body))
    with args.output.open('x') as output, tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    raise RuntimeError('server startup failed')
                try:
                    wire.runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(0.02)
            else:
                raise TimeoutError('server startup')
            began = time.monotonic()
            source = join(b'revocation')
            for _ in range(args.audience):
                if time.monotonic() - began > 60:
                    raise TimeoutError('setup budget')
                member = join(b're_globl')
                # Independent encoded CBOR: authenticate, then observe bit 4.
                request(member, b'\x82\x00\x82\x00pobserver-barrier', b'\x82\x00\x82\x00\xf5')
                request(member, b'\x82\x00\x82\x01\x04', b'\x82\x00\x82\x01\x04')
                audience.append(member)
            observer = join(b're_globl')
            request(observer, b'\x82\x00\x82\x00pobserver-barrier', b'\x82\x00\x82\x00\xf5')
            request(observer, b'\x82\x00\x82\x01\x04', b'\x82\x00\x82\x01\x04')
            probe = join(b'barrier-probe')

            def probe_loop():
                counter = 0
                try:
                    while not probe_stop.is_set():
                        value = encode_uint(counter)
                        began_probe = time.monotonic_ns()
                        probe.socket.sendall(wire.frame(0x82, b'\x82' + encode_uint(probe.id) + value))
                        assert probe.receive() == (2, b'\x82' + encode_uint(probe.id) + value)
                        probe_samples.append({'sent_ns': began_probe, 'received_ns': time.monotonic_ns()})
                        probe_started.set()
                        counter += 1
                        probe_stop.wait(0.001)
                except Exception:
                    probe_errors.append(traceback.format_exc())
                    probe_started.set()
            probe_thread = threading.Thread(target=probe_loop)
            probe_thread.start()
            assert probe_started.wait(2) and (not probe_errors)
            source.close()
            began = time.monotonic_ns()
            result['barrier_start_ns'] = began
            observer.socket.sendall(wire.frame(0x82, b'\x82\x00\x82\x01\x00'))
            notices = 0
            for _ in range(2):
                opcode, body = observer.receive()
                assert opcode == 2, (opcode, body.hex())
                if body == b'\x82\x00\x82\x01\x00':
                    result['sequence'].append('unsubscribe_ack')
                    break
                notice(body, source.id)
                notices += 1
                result['sequence'].append('departure')
            else:
                raise AssertionError('missing unsubscribe acknowledgment')
            result['barrier_end_ns'] = time.monotonic_ns()
            result['unsubscribe_ack_ms'] = (result['barrier_end_ns'] - began) / 1000000.0
            # Witness timing excludes exhaustive audience verification/teardown.
            time.sleep(0.01)
            probe_stop.set()
            probe_thread.join(timeout=6)
            assert not probe_thread.is_alive() and (not probe_errors)
            result['probe_samples'] = probe_samples
            overlap = [x for x in probe_samples if x['received_ns'] >= began and x['sent_ns'] <= result['barrier_end_ns']]
            result['probe_overlap_count'] = len(overlap)
            result['probe_max_ms'] = max(((x['received_ns'] - x['sent_ns']) / 1000000.0 for x in probe_samples))
            result['probe_overlap_max_ms'] = max(((x['received_ns'] - x['sent_ns']) / 1000000.0 for x in overlap), default=None)
            for peer in audience:
                departure(peer, source.id)
            observer.socket.settimeout(0.1)
            try:
                opcode, body = observer.receive()
                assert opcode == 2, (opcode, body.hex())
                notice(body, source.id)
                notices += 1
                result['sequence'].append('late_departure')
            except TimeoutError:
                pass
            observer.socket.settimeout(5)
            assert notices == 1, 'original notice missing or duplicated'
            request(observer, b'\x82\x00\x82\x01\x04', b'\x82\x00\x82\x01\x04')
            fresh = join(b'revocation')
            fresh.close()
            departure(observer, fresh.id)
            for peer in audience:
                departure(peer, fresh.id)
            result['resubscribe_verified'] = True
            observer.close()
            # A one-slot barrier test should not implicitly demand two queued
            # departures per unrelated recipient. Burst mode is a separate
            # pressure diagnostic and may legitimately overload that setting.
            if args.burst_teardown:
                probe.close()
            for peer in audience:
                departure(peer, observer.id)
            if not args.burst_teardown:
                probe.close()
            for peer in audience:
                departure(peer, probe.id)
            for _ in range(100):
                metrics = wire.runtime.metrics(admin)
                if int(metrics['war_sessions']) == args.audience and int(metrics['war_queued_bytes']) == 0:
                    break
                time.sleep(0.01)
            result['after'] = metrics
            assert int(metrics['war_sessions']) == args.audience and int(metrics['war_queued_bytes']) == 0
            assert int(metrics['war_routing_copied_bytes_total']) == 0
            for key in wire.ZERO_ERRORS:
                assert int(metrics[key]) == 0, (key, metrics[key])
            result['passed'] = result['sequence'] == ['departure', 'unsubscribe_ack']
            result['departure_checks'] = 4 * args.audience + 2
        except Exception:
            result['error'] = traceback.format_exc()
        finally:
            probe_stop.set()
            if probe_thread is not None:
                probe_thread.join(timeout=6)
            if server.poll() is None:
                shutdown_began = time.monotonic_ns()
                server.terminate()
                result['shutdown_closes'] = 0
                try:
                    for peer in audience:
                        opcode, body = peer.receive()
                        assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1001, (opcode, body.hex())
                        result['shutdown_closes'] += 1
                    result['shutdown_close_verified'] = result['shutdown_closes'] == args.audience
                except Exception:
                    result['shutdown_error'] = traceback.format_exc()
                for peer in peers:
                    peer.socket.close()
                try:
                    server.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    result['server_forced_kill'] = True
                    server.kill()
                    server.wait(timeout=3)
                result['shutdown_check_elapsed_ms'] = (time.monotonic_ns() - shutdown_began) / 1000000.0
            result['server_exit'] = server.returncode
            if server.returncode != 0 or result['server_forced_kill'] or (not result.get('shutdown_close_verified')):
                result['passed'] = False
            log.seek(0)
            result['server_log'] = log.read().decode(errors='replace')
        json.dump(result, output, indent=2)
        output.write('\n')
    print(json.dumps({k: result.get(k) for k in ['passed', 'sequence', 'unsubscribe_ack_ms', 'resubscribe_verified', 'probe_max_ms', 'probe_overlap_count', 'probe_overlap_max_ms', 'shutdown_closes', 'shutdown_check_elapsed_ms', 'server_exit', 'error']}))
    return 0 if result['passed'] else 1
if __name__ == '__main__':
    raise SystemExit(main())
