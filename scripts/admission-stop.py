#!/usr/bin/env python3
"""Verify native shutdown with an established CBOR peer during a refused-TCP flood.

An independent raw-frame peer verifies echoes and shutdown Close/EOF. The Rust
flood client owns no admitted session and is expected to fail when TCP stops.
These are cancellation diagnostics, not ordinary throughput measurements.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import tempfile
import time
import traceback

import benchmark as runtime

spec = importlib.util.spec_from_file_location('accept_pressure', Path(__file__).with_name('accept-pressure.py'))
pressure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pressure)


def run(args):
    result = {'scope': __doc__, 'workers': args.workers, 'flood_workers': args.flood_workers,
              'server_sha256': hashlib.sha256(args.server.read_bytes()).hexdigest(),
              'client_sha256': hashlib.sha256(args.client.read_bytes()).hexdigest(),
              'source': Path(__file__).read_text(), 'passed': False}
    native = client = healthy = management = None
    with tempfile.TemporaryFile() as native_log, tempfile.TemporaryFile() as flood_log:
        try:
            port, admin = runtime.free_port(), runtime.free_port()
            assert port != admin
            command = [str(args.server.resolve()), '--workers', str(args.workers), '--max-connections', '1',
                       '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--ping-interval-secs', '0']
            flood = [str(args.client.resolve()), '--url', f'ws://127.0.0.1:{port}', '--tcp', f'127.0.0.1:{port}',
                     '--flood-only', '--seconds', '15', '--flood-workers', str(args.flood_workers)]
            env = {key: value for key, value in os.environ.items() if not key.startswith('WAR_') and key != 'LD_PRELOAD'}
            result.update(server_command=command, client_command=flood)
            native = subprocess.Popen(command, env=env, stdout=native_log, stderr=native_log)
            management = pressure.wait_live(native, lambda: pressure.Management(admin))
            healthy = pressure.Client(socket.create_connection(('127.0.0.1', port), timeout=3), False)
            client = subprocess.Popen(flood, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=flood_log, text=True, bufsize=1)
            assert select.select([client.stdout], [], [], 5)[0], 'missing flood ready gate'
            ready = json.loads(client.stdout.readline())
            assert ready['event'] == 'ready' and ready['details']['joins'] == 0
            client.stdin.write('continue\n'); client.stdin.flush()
            started = time.monotonic()
            while True:
                assert native.poll() is None and client.poll() is None
                observed = management.metrics()
                if observed['war_rejected_total'] >= 10000 and time.monotonic() - started >= .5:
                    break
                assert time.monotonic() - started < 5, 'flood did not exercise admission'
                time.sleep(.01)
            result['warm_metrics'] = observed
            for _ in range(8):
                healthy.echo()
            observed = management.metrics()
            result['before_signal_metrics'] = observed
            assert observed['war_rejected_total'] > result['warm_metrics']['war_rejected_total']
            assert observed['war_connections'] == observed['war_sessions'] == 1
            assert observed['war_incoming_messages_total'] == 9
            assert client.poll() is None and not select.select([client.stdout], [], [], 0)[0], 'flood ended before shutdown'
            started = time.monotonic()
            native.send_signal(signal.SIGTERM)
            opcode, body = healthy.read()
            assert opcode == 8 and int.from_bytes(body[:2], 'big') == 1001
            # Shutdown intentionally flushes Close and retires TCP; no late ACK.
            assert healthy.stream.recv(1) == b''
            assert native.wait(timeout=5) == 0
            result.update(shutdown_close=1001, shutdown_seconds=time.monotonic() - started,
                          verified_echoes_during_flood=8, native_exit_code=0)
            assert client.wait(timeout=8) == 1
            flood_log.seek(0)
            error = flood_log.read().decode(errors='replace')
            assert any(message in error for message in ['Connection refused', 'Connection reset by peer',
                                                        'Software caused connection abort', 'deadline has elapsed']), error
            result.update(expected_flood_exit_code=1, flood_log=error, passed=True)
        except Exception as error:
            result.update(error=repr(error), traceback=traceback.format_exc())
        finally:
            if healthy is not None:
                healthy.stream.close()
            if management is not None:
                management.close()
            for process, label in [(client, 'flood'), (native, 'native')]:
                if process is None:
                    continue
                if process.poll() is None:
                    process.send_signal(signal.SIGTERM)
                try:
                    result[f'{label}_exit_code'] = process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill(); result[f'{label}_exit_code'] = process.wait(timeout=5)
                    result['passed'] = False
                    result[f'{label}_forced_kill'] = True
        native_log.seek(0); flood_log.seek(0)
        result.update(native_log=native_log.read().decode(errors='replace'), flood_log=flood_log.read().decode(errors='replace'))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=runtime.SERVER)
    parser.add_argument('--client', type=Path, default=runtime.ROOT / 'target/release/examples/admission')
    parser.add_argument('--workers', type=int, choices=[1, 4], default=1)
    parser.add_argument('--flood-workers', type=int, default=256)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = run(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: result.get(key) for key in ['passed', 'workers', 'shutdown_seconds', 'native_exit_code', 'flood_exit_code', 'error']}))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
