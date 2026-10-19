"""Real native/client children; controlled perf ignores SIGINT; test-only grace acceleration."""
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile

ROOT = Path.cwd()
output = ROOT / 'artifacts/inspection-perf-cleanup-20261019-stuck.json'
assert not output.exists()
original = subprocess.Popen
children = []


class TrackedChild(original):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        children.append(self)

    def wait(self, timeout=None):
        if isinstance(self.args, list) and self.args[0] == 'perf' and timeout == 10:
            timeout = .05
        return super().wait(timeout)


record = {'passed': False, 'scope': __doc__}
with tempfile.TemporaryDirectory() as temporary:
    perf = Path(temporary) / 'perf'
    perf.write_text('#!/usr/bin/env python3\nimport signal,time\nsignal.signal(signal.SIGINT, signal.SIG_IGN)\nprint("controlled perf ignores SIGINT", flush=True)\ntime.sleep(30)\n')
    perf.chmod(0o755)
    old_path, old_argv = os.environ['PATH'], sys.argv
    try:
        os.environ['PATH'] = temporary + os.pathsep + old_path
        subprocess.Popen = TrackedChild
        sys.path.insert(0, str(ROOT / 'scripts'))
        sys.argv = [str(ROOT / 'scripts/inspection.py'), '--server', str(ROOT / 'artifacts/candidates/worker-count-20261019-current/war-relay'), '--client', str(ROOT / 'artifacts/candidates/private-payload-20261019-client/inspection'), '--client-source-root', str(ROOT / 'artifacts/private-payload-20261019-work'), '--mode', 'none', '--rooms', '0', '--seconds', '1', '--inspectors', '0', '--pairs', '2', '--churn-workers', '0', '--workers', '4', '--client-workers', '4', '--server-cpus', '', '--client-cpus', '', '--profile', '--output', str(output)]
        try:
            runpy.run_path(sys.argv[0], run_name='__main__')
        except SystemExit as error:
            record['cli_exit_code'] = error.code
        record['children_before_fallback'] = [{'pid': child.pid, 'command': child.args[0], 'exit_code': child.poll()} for child in children]
        receipt = json.loads(output.read_text())
        assert record['cli_exit_code'] == 1 and not receipt['passed']
        assert receipt['perf_forced_kill'] and receipt['perf_exit_code'] == -signal.SIGKILL
        assert receipt['client_exit_code'] == -signal.SIGKILL and receipt['server_exit_code'] == 0
        assert all(child.poll() is not None for child in children)
        assert 'controlled perf ignores SIGINT' in receipt['perf_log']
        record['passed'] = True
    finally:
        subprocess.Popen, os.environ['PATH'], sys.argv = original, old_path, old_argv
        record['fallback_killed_pids'] = []
        for child in reversed(children):
            if child.poll() is None:
                record['fallback_killed_pids'].append(child.pid)
                child.kill()
                child.wait(timeout=3)
        (ROOT / 'artifacts/inspection-perf-cleanup-20261019-probe.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
