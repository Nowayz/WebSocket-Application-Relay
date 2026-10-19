"""Server-only allocator diagnostic around verified bridge memory gates."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import inspection
parser=argparse.ArgumentParser()
parser.add_argument('--bridge',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
parser.add_argument('--heap-prefix',type=Path,required=True)
args=parser.parse_args()
prefix=args.heap_prefix.resolve()
assert not os.path.lexists(args.output) and not list(prefix.parent.glob(prefix.name+'*.heap'))
library=Path('/usr/lib/libtcmalloc.so').resolve()
settings={'LD_PRELOAD':str(library),'HEAPPROFILE':str(prefix),'HEAPPROFILESIGNAL':str(signal.SIGUSR2),
          'HEAP_PROFILE_ALLOCATION_INTERVAL':str(100*1024**3),'HEAP_PROFILE_INUSE_INTERVAL':str(100*1024**3)}
spec=importlib.util.spec_from_file_location('census',ROOT/'artifacts/bridge-buffer-memory-probe.py')
census=importlib.util.module_from_spec(spec);spec.loader.exec_module(census)
original_popen,original_memory=subprocess.Popen,census.memory
bridge_pid=None; captures=[]
sha=lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest()
def launch(command,*positional,**kwargs):
    global bridge_pid
    bridge=str(args.bridge.resolve()) in command
    if bridge:kwargs['env']={**kwargs['env'],**settings}
    process=original_popen(command,*positional,**kwargs)
    if bridge:bridge_pid=process.pid
    return process

def memory(pid):
    result=original_memory(pid)
    if pid==bridge_pid:
        capture=inspection.dump_heap(pid,prefix)
        capture['phase']=['baseline','joined','warmed','retired'][len(captures)]
        captures.append(capture);result['heap']=capture
        result['fds']=len(list(Path(f'/proc/{pid}/fd').iterdir()))
    return result
try:
    subprocess.Popen,census.memory=launch,memory
    result=census.run(SimpleNamespace(server=ROOT/'artifacts/candidates/connection-slots-selected/war-relay',
       bridge=args.bridge,clients=256,rounds=8,output=args.output))
finally:
    subprocess.Popen,census.memory=original_popen,original_memory
    if args.output.exists():
        receipt=json.loads(args.output.read_text())
        receipt['profile_diagnostic']=True
        receipt['heap_profiler']={'scope':'Only bridge allocator replaced; all timings and RSS excluded from ordinary comparisons. Quiescent gate captures of requested heap, not normal RSS.',
          'library':str(library),'library_sha256':sha(library),'environment':settings,'captures':captures,
          'wrapper_sha256':sha(__file__),'base_runner_sha256':sha(ROOT/'artifacts/bridge-buffer-memory-probe.py'),
          'dump_helper_sha256':sha(ROOT/'scripts/inspection.py')}
        args.output.write_text(json.dumps(receipt,indent=2)+'\n')
assert result['passed'] and len(captures)==4
assert result['baseline']['bridge']['fds']==result['retired']['bridge']['fds']
print(json.dumps({'passed':result['passed'],'captures':len(captures),'receipt':str(args.output)}))
