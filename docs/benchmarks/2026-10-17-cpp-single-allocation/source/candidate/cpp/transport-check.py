#!/usr/bin/env python3
"""Report transport compatibility differences without weakening the shared corpus."""
import concurrent.futures, importlib.util, json, os, pathlib, subprocess, time, traceback
ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
port, admin = (c.wire.runtime.free_port(), c.wire.runtime.free_port())
env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
env['WAR_ADMIN_TOKEN'] = c.TOKEN
cmd = [str(ROOT / 'artifacts/candidates/cpp-modern/war-relay-cpp'), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4', '--max-message-bytes', str(c.LIMIT)]
original = c.Peer.__init__

def init(self, *args):
    original(self, *args)
    self.socket.settimeout(0.5)
c.Peer.__init__ = init
cases = [case for case in c.corpus() if case['stage'] not in ('value', 'join')]

def exercise(index, case):
    worker = None
    row = {'name': case['name'], 'stage': case['stage'], 'passed': False}
    try:
        worker = c.Worker(port, index + 1000, False)
        worker.run([(case, 'whole')])
        row['passed'] = True
    except Exception as e:
        row['difference'] = str(e)
    finally:
        if worker:
            for peer in worker.peers:
                peer.socket.close()
    return row
with open(ROOT / 'artifacts/cpp-modern-transport-server.log', 'w') as log:
    server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    try:
        for _ in range(200):
            try:
                c.metrics(admin)
                break
            except OSError:
                time.sleep(0.01)
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            rows = list(pool.map(lambda item: exercise(*item), enumerate(cases)))
    finally:
        server.terminate()
        exitcode = server.wait(timeout=10)
record = {'scope': 'Original Rust corpus expectations, unmodified. Whole-frame transport probes only; application CBOR corpus is checked separately. Differences are recorded as differences, not accepted as equivalent behavior.', 'cases': rows, 'passed_cases': sum((r['passed'] for r in rows)), 'total_cases': len(rows), 'server_exit_code': exitcode}
(ROOT / 'artifacts/cpp-modern-transport.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record, indent=2))
