#!/usr/bin/env python3
"""Short Linux loopback probe: overload must release global/IP admission and output."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('admission_reuse', Path(__file__).with_name('admission-reuse.py'))
helpers = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    log_path = args.output.with_suffix('.server.log')
    for path in [args.output, log_path]:
        if os.path.lexists(path):
            parser.error(f'output already exists; choose a fresh prefix: {path}')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    port, admin = helpers.free_port(), helpers.free_port()
    while admin == port:
        admin = helpers.free_port()
    command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}',
               '--workers', '4', '--max-connections', '3', '--max-connections-per-ip', '1',
               '--max-message-bytes', '65536', '--queue-bytes', '65546', '--queue-messages', '2',
               '--total-queue-bytes', '262144', '--tcp-send-buffer-bytes', '4096',
               '--messages-per-second', '0', '--bytes-per-second', '0', '--ping-interval-secs', '0']
    record = {'passed': False, 'command': command, 'cases': [],
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'source_sha256': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in
                                [Path(__file__), Path(helpers.__file__), ROOT/'scripts/input-corpus.py', ROOT/'scripts/fragment-fairness.py']}}
    peers = []
    env = {k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
    env['WAR_ADMIN_TOKEN'] = 'overload-reuse-fixture'
    with args.output.open('x') as receipt, log_path.open('x') as log:
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        began = time.monotonic()
        def metrics():
            request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization':'Bearer overload-reuse-fixture'})
            with urllib.request.urlopen(request, timeout=2) as response:
                return {k:int(v) for k,v in (row.split() for row in response.read().decode().splitlines() if row.startswith('war_'))}
        def wait_for(predicate):
            deadline = time.monotonic() + 3
            while True:
                state = metrics()
                if predicate(state): return state
                assert process.poll() is None and time.monotonic() < deadline, state
                time.sleep(.005)
        controls = pressure_messages = 0
        sequence = 0
        def echo(peer):
            nonlocal sequence, controls
            sequence = sequence % 255 + 1
            helpers.self_delivery(peer, sequence)
            controls += 1
        def join(source, channel):
            peer = helpers.upgraded(port, source)
            peers.append(peer)
            peer.join(channel)
            return peer
        try:
            while True:
                assert process.poll() is None, 'server stopped during startup'
                try: metrics(); break
                except OSError:
                    assert time.monotonic() - began < 3, 'server startup deadline'
                    time.sleep(.005)
            time.sleep(.01)  # Let the startup HTTP connection finish closing.
            baseline_fds = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
            record['baseline_fds'] = baseline_fds
            witness = join('127.0.0.1', b'overload-witness')
            sender = join('127.0.0.3', b'overload-source')
            for wave in range(3):
                # Leave global room available, so this refusal proves IP quota enforcement.
                with socket.create_connection(('127.0.0.1',port), timeout=3, source_address=('127.0.0.1',0)) as refused:
                    assert helpers.eof(refused) == b''
                ip_refused = wait_for(lambda m: m['war_ip_rejected_total'] == wave + 1)
                assert ip_refused['war_connections'] == 2
                slow = join('127.0.0.2', b'overload-sink')
                slow.socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
                before = wait_for(lambda m: m['war_connections'] == m['war_sessions'] == 3)
                with socket.create_connection(('127.0.0.1',port), timeout=3, source_address=('127.0.0.4',0)) as refused:
                    assert helpers.eof(refused) == b''
                rejected = wait_for(lambda m: m['war_rejected_total'] == before['war_rejected_total'] + 1)
                assert rejected['war_connections'] == 3
                sent, pressure_samples = [], []
                for index in range(64):
                    # At most 2 MiB submitted, over 30x the session queue budget.
                    value = b'\x59\x80\x00' + bytes([wave, index]) + b'Z' * (32768 - 2)
                    sender.socket.sendall(helpers.corpus.wire.frame(0x82, b'\x82\x1b' + slow.id.to_bytes(8,'big') + value))
                    sent.append(value); pressure_messages += 1
                    echo(witness)
                    state = metrics()
                    assert state['war_queued_bytes'] <= 262144
                    pressure_samples.append({k:state[k] for k in ['war_queued_bytes','war_connections','war_sessions','war_slow_consumers_total']})
                    if state['war_slow_consumers_total'] == wave + 1: break
                else: raise AssertionError('bounded pressure did not overload the unread peer')
                echo(sender)  # FIFO barrier: publisher consumed all prior pressure input.
                overloaded = wait_for(lambda m: m['war_sessions'] == 2)
                assert overloaded['war_slow_consumers_total'] == wave + 1
                buffered = 0
                terminal = None
                try:
                    while True:
                        opcode, body = slow.receive()
                        if opcode == 8:
                            assert int.from_bytes(body[:2],'big') == 1013, body
                            slow.socket.sendall(helpers.corpus.wire.frame(0x88, body))
                            assert helpers.eof(slow.socket) == b''
                            terminal = 'close-1013'
                            break
                        assert opcode == 2 and body[0] == 0x82
                        identity, offset = helpers.corpus.wire.uint(body, 1)
                        assert identity == sender.id and body[offset:] == sent[buffered], 'buffered payload/FIFO changed'
                        buffered += 1
                except (EOFError, ConnectionResetError):
                    # A partially written frame cannot be followed by a Close.
                    terminal = 'transport-ended'
                finally: slow.socket.close()
                released = wait_for(lambda m: m['war_connections'] == m['war_sessions'] == 2 and m['war_queued_bytes'] == 0)
                replacement = join('127.0.0.2', b'overload-next')
                echo(replacement); echo(witness)
                helpers.close_peer(replacement)
                wait_for(lambda m: m['war_connections'] == 2)
                record['cases'].append({'wave':wave, 'ip_quota_refused':True, 'global_quota_refused':True, 'pressure_samples':pressure_samples, 'pressure_messages':len(sent), 'buffered_frames_verified':buffered,
                                        'terminal':terminal, 'healthy_packets_during_pressure':len(sent), 'slow_id':slow.id, 'replacement_id':replacement.id,
                                        'overloaded':overloaded, 'released':released, 'same_ip_rejoined_and_routed':True})
            helpers.close_peer(sender); helpers.close_peer(witness)
            final = wait_for(lambda m: m['war_connections'] == 0)
            for key in ['war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes']:
                assert final[key] == 0, (key, final[key])
            deadline = time.monotonic() + 3
            while True:
                final_fds = len(list(Path(f'/proc/{process.pid}/fd').iterdir()))
                if final_fds == baseline_fds: break
                assert time.monotonic() < deadline, (baseline_fds, final_fds)
                time.sleep(.005)
            record['final_fds'] = final_fds
            assert final['war_channels'] == 1
            assert final['war_accepted_total'] == 14 and final['war_joined_total'] == final['war_disconnected_total'] == 8
            assert final['war_rejected_total'] == 6 and final['war_slow_consumers_total'] == 3
            assert final['war_ip_rejected_total'] == 3
            identities = [c[k] for c in record['cases'] for k in ['slow_id', 'replacement_id']]
            assert len(set(identities + [witness.id, sender.id])) == 8
            assert final['war_incoming_messages_total'] == pressure_messages + controls
            assert final['war_incoming_bytes_total'] == pressure_messages * 32781 + controls * 12
            for key in ['war_protocol_errors_total','war_global_pressure_total','war_rate_limited_total','war_handshake_errors_total','war_write_timeouts_total']:
                assert final[key] == 0, (key, final[key])
            record.update(passed=True, final_metrics=final, pressure_messages=pressure_messages, checked_control_messages=controls)
        except BaseException as error:
            record['error'] = repr(error)
            raise
        finally:
            for peer in peers: peer.socket.close()
            if process.poll() is None: process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
            record['exit_code'] = process.returncode
            record['passed'] = record['passed'] and process.returncode == 0
            record['elapsed_seconds'] = time.monotonic() - began
            receipt.write(json.dumps(record,indent=2)+'\n')
        print(json.dumps(record))
        if not record['passed']: raise SystemExit(1)


if __name__ == '__main__':
    main()
