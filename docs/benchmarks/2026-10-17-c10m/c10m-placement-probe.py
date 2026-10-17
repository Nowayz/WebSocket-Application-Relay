"""Disposable placement experiment; only pins the fresh capacity server's workers."""
import hashlib
import os
from pathlib import Path
import runpy
import sys

root = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(root / 'scripts'))
import benchmark

pin = '--pin-workers' in sys.argv
if pin:
    sys.argv.remove('--pin-workers')
original = benchmark.snapshot
seen = set()
def snapshot(pid):
    placement = {}
    for path in Path(f'/proc/{pid}/task').iterdir():
        try:
            name = (path / 'comm').read_text().strip()
            if not name.startswith('war-io-'):
                continue
            tid = int(path.name)
            before = sorted(os.sched_getaffinity(tid))
            if pin and tid not in seen:
                os.sched_setaffinity(tid, {before[int(name.removeprefix('war-io-'))]})
                seen.add(tid)
            placement[name] = {'tid': tid, 'before': before, 'after': sorted(os.sched_getaffinity(tid))}
        except FileNotFoundError:
            pass
    result = original(pid)
    result['worker_placement'] = placement
    result['placement_probe_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    return result
benchmark.snapshot = snapshot
runpy.run_path(str(root / 'scripts/capacity.py'), run_name='__main__')
