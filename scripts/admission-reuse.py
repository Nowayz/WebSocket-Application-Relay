#!/usr/bin/env python3
"""Short Linux loopback check of native global/IP admission after failed upgrades.

Uses 127.0.0.1 for a healthy witness and 127.0.0.2 for refusal/recovery traffic.
Only disposable server processes and fresh output files are used.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
corpus = importlib.util.module_from_spec(spec)
spec.loader.exec_module(corpus)


def free_port():
    with socket.socket() as stream:
        stream.bind(('127.0.0.1', 0))
        return stream.getsockname()[1]


def upgraded(port, source):
    return corpus.Peer(port, source_address=(source, 0))


def self_delivery(peer, sequence):
    value = b'\x18' + bytes([sequence])
    peer.socket.sendall(corpus.wire.frame(0x82, b'\x82\x1b' + peer.id.to_bytes(8, 'big') + value))
    opcode, body = peer.receive()
    assert opcode == 2 and body[0] == 0x82
    sender, offset = corpus.wire.uint(body, 1)
    assert sender == peer.id and body[offset:] == value, 'self delivery changed'


def close_peer(peer):
    peer.socket.sendall(corpus.wire.frame(0x88, b'\x03\xe8'))
    assert peer.receive() == (8, b'\x03\xe8')
    assert peer.socket.recv(1) == b''
    peer.socket.close()


def eof(stream):
    result = bytearray()
    while True:
        try:
            chunk = stream.recv(4096)
        except ConnectionResetError:
            return bytes(result)
        if not chunk:
            return bytes(result)
        result.extend(chunk)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=ROOT / 'target/release/war-relay')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    # Check the complete evidence group before creating either file. Exclusive
    # opens still prevent overwrites if another process wins a creation race.
    for artifact in [args.output, args.output.with_suffix('.server.log')]:
        if os.path.lexists(artifact):
            parser.error(f'output already exists; choose a fresh prefix: {artifact}')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as receipt, args.output.with_suffix('.server.log').open('x') as log:
        port, admin = free_port(), free_port()
        while admin == port:
            admin = free_port()
        command = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}',
                   '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4',
                   '--max-connections', '3', '--max-connections-per-ip', '1',
                   '--handshake-timeout-secs', '1', '--ping-interval-secs', '0']
        env = {k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
        env['WAR_ADMIN_TOKEN'] = 'admission-reuse-fixture'
        record = {'passed': False, 'command': command, 'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
                  'runner_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  'corpus_helper_sha256': hashlib.sha256((ROOT / 'scripts/input-corpus.py').read_bytes()).hexdigest(), 'cases': []}
        process = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        peers = []
        raw = None
        def metrics():
            request = urllib.request.Request(f'http://127.0.0.1:{admin}/metrics', headers={'Authorization':'Bearer admission-reuse-fixture'})
            with urllib.request.urlopen(request, timeout=2) as response:
                return dict((k,int(v)) for k,v in (line.split() for line in response.read().decode().splitlines() if line.startswith('war_')))
        def wait_count(expected):
            deadline = time.monotonic() + 3
            while True:
                state = metrics()
                if state['war_connections'] == expected:
                    return state
                assert time.monotonic() < deadline, (expected,state)
                time.sleep(.005)
        began = time.monotonic()
        try:
            while True:
                assert process.poll() is None
                try:
                    metrics()
                    break
                except OSError:
                    assert time.monotonic() - began < 3
                    time.sleep(.005)
            witness = upgraded(port, '127.0.0.1'); peers.append(witness)
            witness.join(b'admission-good')
            sequence = witness_messages = 0
            for kind in ['malformed-http', 'early-eof', 'timeout'] * 2:
                before = wait_count(1)
                raw = socket.create_connection(('127.0.0.1',port), timeout=3, source_address=('127.0.0.2',0))
                wait_count(2)
                with socket.create_connection(('127.0.0.1',port), timeout=3, source_address=('127.0.0.2',0)) as refused:
                    assert eof(refused) == b''
                rejected = wait_count(2)
                assert rejected['war_ip_rejected_total'] == before['war_ip_rejected_total'] + 1
                if kind == 'malformed-http':
                    raw.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\n\r\n')
                elif kind == 'early-eof':
                    raw.shutdown(socket.SHUT_WR)
                deadline = time.monotonic() + 3
                during_wait = 0
                while not select.select([raw],[],[],.02)[0]:
                    sequence = sequence % 255 + 1
                    self_delivery(witness, sequence); witness_messages += 1; during_wait += 1
                    assert time.monotonic() < deadline, kind
                response = eof(raw)
                # Native failed upgrades may retire without an HTTP error body.
                assert not response or response.startswith(b'HTTP/1.1 400 '), response
                raw.close(); raw = None
                released = wait_count(1)
                assert released['war_handshake_errors_total'] == before['war_handshake_errors_total'] + 1
                replacement = upgraded(port, '127.0.0.2'); peers.append(replacement)
                replacement.join(b'admission-next')
                self_delivery(replacement, 42)
                close_peer(replacement)
                sequence = sequence % 255 + 1
                self_delivery(witness, sequence); witness_messages += 1
                wait_count(1)
                record['cases'].append({'kind':kind,'ip_refusal':True,'slot_reused':True,'replacement_routed':True,'healthy_routed':True,'healthy_packets_during_wait':during_wait})
            close_peer(witness)
            final = wait_count(0)
            for key in ['war_sessions','war_queued_bytes','war_variable_bytes','war_output_queue_capacity_bytes','war_read_buffer_reserved_bytes']:
                assert final[key] == 0, (key,final[key])
            assert final['war_ip_rejected_total'] == final['war_handshake_errors_total'] == 6
            assert final['war_accepted_total'] == 19 and final['war_joined_total'] == final['war_disconnected_total'] == 7
            assert final['war_incoming_messages_total'] == witness_messages + 6
            assert final['war_deliveries_total'] == sum(peer.data_frames for peer in peers)
            assert final['war_delivered_bytes_total'] == sum(peer.received_data_bytes for peer in peers)
            assert all(case['healthy_packets_during_wait'] > 0 for case in record['cases'] if case['kind'] == 'timeout')
            record.update(passed=True, final_metrics=final, witness_messages=witness_messages)
        except BaseException as error:
            record['error'] = repr(error)
            raise
        finally:
            if raw is not None: raw.close()
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
