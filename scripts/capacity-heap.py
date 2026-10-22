#!/usr/bin/env python3
"""Repeat a capacity receipt with phase-gated live allocations and socket accounting.

This is an allocation diagnostic, not an ordinary throughput/RSS comparison.
The client binary and workload are copied from the supplied receipt. Only the
server receives the replacement allocator. No host sysctls or limits change.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

import capacity
import inspection


PATH_ARGS = ('server', 'client', 'output', 'server_preload', 'user_latency_output',
             'user_latency_histogram_output', 'pressure_client', 'kernel_slab_sizes')


def signal_state(pid):
    mask = 1 << (signal.SIGUSR2 - 1)
    active = pending = False
    for path in Path(f'/proc/{pid}/task').glob('*/status'):
        try:
            for line in path.read_text().splitlines():
                key, _, value = line.partition(':')
                if key == 'SigBlk':
                    active |= bool(int(value.strip(), 16) & mask)
                elif key in ('SigPnd', 'ShdPnd'):
                    pending |= bool(int(value.strip(), 16) & mask)
        except FileNotFoundError:
            pass  # A thread can retire during this read-only observation.
    return active, pending


def capture(pid, prefix, timeout):
    start = time.monotonic()
    deadline = start + timeout
    while any(signal_state(pid)):
        if time.monotonic() >= deadline:
            raise TimeoutError('previous heap signal remains active or pending')
        time.sleep(.025)
    before = set(prefix.parent.glob(prefix.name + '*.heap'))
    attempts = 0
    previous = None
    while time.monotonic() < deadline:
        active, pending = signal_state(pid)
        created = sorted(set(prefix.parent.glob(prefix.name + '*.heap')) - before)
        if created:
            path = created[-1]
            data = path.read_bytes()
            # Raced retries can create several valid snapshots of this same
            # paused gate. Retain their paths and select the latest complete one.
            if (data == previous and data.endswith(b'\n') and b'MAPPED_LIBRARIES:' in data
                    and not any(signal_state(pid))):
                return {'path': str(path), 'sha256': hashlib.sha256(data).hexdigest(),
                        'header': data.splitlines()[0].decode(),
                        'signal_attempts': attempts, 'capture_seconds': time.monotonic() - start,
                        'gate_profiles': [str(value) for value in created]}
            previous = data
        elif not active and not pending:
            # gperftools' signal handler uses TryLock and can intentionally
            # ignore a request while another thread holds its heap lock.
            os.kill(pid, signal.SIGUSR2)
            attempts += 1
        time.sleep(.025)
    raise TimeoutError(f'heap gate incomplete after {attempts} signal requests')


def run(options):
    source = json.loads(options.receipt.read_text())
    values = source.get('args', source).copy()
    values.update(output=options.output, server_preload=options.library,
                  kernel_socket_resources=True, kernel_slab_sizes=options.slab_sizes)
    for key in ('profile', 'profile_counters', 'profile_client', 'profile_scheduling', 'profile_kernel'):
        values[key] = False
    for key in PATH_ARGS:
        if values.get(key) is not None:
            values[key] = Path(values[key])
    args = argparse.Namespace(**values)
    prefix = options.prefix.resolve()
    if options.output.exists():
        raise FileExistsError(options.output)
    prefix.parent.mkdir(parents=True, exist_ok=True)
    if list(prefix.parent.glob(prefix.name + '*.heap')):
        raise FileExistsError(f'use a fresh heap prefix: {prefix}')
    if not options.library.is_file():
        raise FileNotFoundError(options.library)
    environment = {'HEAPPROFILE': str(prefix), 'HEAPPROFILESIGNAL': str(signal.SIGUSR2),
                   'HEAP_PROFILE_ALLOCATION_INTERVAL': '0',
                   'HEAP_PROFILE_INUSE_INTERVAL': '0',
                   'HEAP_PROFILE_DEALLOCATION_INTERVAL': '0',
                   'HEAP_PROFILE_TIME_INTERVAL': '0'}
    saved = {key: os.environ.get(key) for key in environment}
    original_launch, original_process = subprocess.Popen, capacity.process
    server_pid = None

    def launch(command, *positional, **keywords):
        nonlocal server_pid
        process = original_launch(command, *positional, **keywords)
        if str(args.server.resolve()) in [str(token) for token in command]:
            server_pid = process.pid
        return process

    def sample(pid, socket_inodes=None):
        result = original_process(pid, socket_inodes)
        if pid == server_pid and socket_inodes is not None:
            result['heap'] = capture(pid, prefix, options.capture_timeout)
        return result

    os.environ.update(environment)
    subprocess.Popen, capacity.process = launch, sample
    try:
        capacity.run(args)
    finally:
        subprocess.Popen, capacity.process = original_launch, original_process
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        if options.output.exists():
            result = json.loads(options.output.read_text())
            result['profile_diagnostic'] = True
            result['heap_diagnostic'] = {
                'scope': 'Replacement allocator; client waits at every captured gate. Exclude CPU, latency and RSS from ordinary comparisons. Logical cleanup can precede deferred QUIC memory release. Kernel counters and socket object estimates are separate, incomplete kernel accounting.',
                'library_sha256': inspection.digest(options.library),
                'controller_sha256': inspection.digest(Path(__file__)),
                'source_receipt': str(options.receipt),
                'capture_timeout_seconds': options.capture_timeout,
                'profiles': [str(path) for path in sorted(prefix.parent.glob(prefix.name + '*.heap'))],
            }
            options.output.write_text(json.dumps(result, indent=2) + '\n')
    if not result['passed']:
        raise RuntimeError(f'capacity verification failed: {options.output}')
    if not all('heap' in event['server'] for event in result['events']):
        raise RuntimeError('missing phase-gate allocation snapshot')
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receipt', type=Path, required=True, help='Existing capacity receipt or its args object')
    parser.add_argument('--output', type=Path, required=True, help='Fresh diagnostic receipt path')
    parser.add_argument('--prefix', type=Path, required=True, help='Fresh heap directory/file prefix')
    parser.add_argument('--library', type=Path, default=Path('/usr/lib/libtcmalloc.so'))
    parser.add_argument('--slab-sizes', type=Path, help='Read-only socket_resources.py --slab-sizes-only receipt')
    parser.add_argument('--capture-timeout', type=float, default=30,
                        help='Diagnostic watchdog per signal capture; does not change relay deadlines')
    options = parser.parse_args()
    if options.capture_timeout <= 0:
        parser.error('capture-timeout must be positive')
    run(options)
