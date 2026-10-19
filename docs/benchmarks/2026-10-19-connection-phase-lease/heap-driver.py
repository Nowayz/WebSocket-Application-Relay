"""Diagnostic allocator capture using the unchanged mixed-population protocol checks."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import inspection

parser = argparse.ArgumentParser()
parser.add_argument('--heap-prefix', type=Path, required=True)
args, forwarded = parser.parse_known_args()
server = Path(forwarded[forwarded.index('--server') + 1]).resolve()
output = Path(forwarded[forwarded.index('--output') + 1])
prefix = args.heap_prefix.resolve()
assert not os.path.lexists(output), 'use a fresh output path'
assert not list(prefix.parent.glob(prefix.name + '*.heap')), 'use a fresh heap prefix'
prefix.parent.mkdir(parents=True, exist_ok=True)
library = Path('/usr/lib/libtcmalloc.so').resolve()
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
settings = {'LD_PRELOAD': str(library), 'HEAPPROFILE': str(prefix),
            'HEAPPROFILESIGNAL': str(signal.SIGUSR2),
            'HEAP_PROFILE_ALLOCATION_INTERVAL': str(100 * 1024**3),
            'HEAP_PROFILE_INUSE_INTERVAL': str(100 * 1024**3)}
spec = importlib.util.spec_from_file_location('mixed_heap', ROOT / 'scripts/mixed-capacity.py')
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)
original_popen, original_snapshot = subprocess.Popen, mixed.snapshot
server_pid = None
full_snapshots = 0
captures = []

def launch(command, *positional, **kwargs):
    global server_pid
    is_server = isinstance(command, list) and str(server) in command
    if is_server:
        kwargs['env'] = {**kwargs['env'], **settings}
    process = original_popen(command, *positional, **kwargs)
    if is_server:
        server_pid = process.pid
    return process

def snapshot(pid, *, include_fds=True):
    global full_snapshots
    result = original_snapshot(pid, include_fds=include_fds)
    if pid == server_pid and include_fds:
        full_snapshots += 1
        # The existing runner takes its first two full server snapshots at the
        # empty baseline and the populated ready gate, before either client runs.
        if full_snapshots in [1, 2]:
            captured = inspection.dump_heap(pid, prefix)
            captured['phase'] = 'baseline' if full_snapshots == 1 else 'population_ready'
            captures.append(captured)
            result['heap'] = captured
    return result

try:
    subprocess.Popen, mixed.snapshot = launch, snapshot
    sys.argv = [str(ROOT / 'scripts/mixed-capacity.py'), *forwarded]
    mixed.main()
finally:
    subprocess.Popen, mixed.snapshot = original_popen, original_snapshot
    if output.exists():
        receipt = json.loads(output.read_text())
        receipt['profile_diagnostic'] = True
        receipt['heap_profiler'] = {
            'scope': 'Server-only replacement allocator. Two quiescent gate dumps; requested live/cumulative heap, not default-allocator RSS or ordinary performance. All timing and RSS excluded from ordinary comparisons.',
            'library': str(library), 'library_sha256': sha(library), 'environment': settings,
            'driver_sha256': sha(__file__), 'dump_helper_sha256': sha(ROOT / 'scripts/inspection.py'),
            'server_pid': server_pid, 'captures': captures}
        output.write_text(json.dumps(receipt, indent=2) + '\n')
assert len(captures) == 2
