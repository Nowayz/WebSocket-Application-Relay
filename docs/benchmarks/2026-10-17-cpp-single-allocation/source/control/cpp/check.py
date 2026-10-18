#!/usr/bin/env python3
"""Exercise the shared independent wire corpus against the experimental C++ server."""
import argparse, concurrent.futures, importlib.util, json, os, pathlib, subprocess, time, traceback, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
parser = argparse.ArgumentParser()
parser.add_argument('--server', type=pathlib.Path, default=ROOT / 'artifacts/candidates/cpp-modern/war-relay-cpp')
parser.add_argument('--prefix', default='cpp-modern-check')
args = parser.parse_args()
port, admin = (c.wire.runtime.free_port(), c.wire.runtime.free_port())
cmd = [str(args.server.resolve()), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4', '--max-message-bytes', str(c.LIMIT)]
env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
env['WAR_ADMIN_TOKEN'] = c.TOKEN
out = {'command': cmd, 'passed': False}
with open(ROOT / ('artifacts/' + args.prefix + '-server.log'), 'w') as log:
    server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    try:
        for _ in range(200):
            try:
                c.metrics(admin)
                break
            except OSError:
                time.sleep(0.01)
        else:
            raise TimeoutError('startup')
        cases = c.corpus()
        work = [(case, mode) for case in cases for mode in ['whole', 'fragmented', 'pipeline'] if mode != 'pipeline' or case['stage'] in ('value', 'after-frame')]
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
            jobs = [pool.submit(c.Worker(port, i, False).run, work[i::4]) for i in range(4)]
            results = [j.result() for j in jobs]
        out.update(cases=len(cases), exchanges=len(work), workers=results)
        for _ in range(200):
            final = c.metrics(admin)
            if final['war_connections'] == 0:
                break
            time.sleep(0.01)
        assert final['war_connections'] == final['war_users'] == final['war_queued_bytes'] == final['war_variable_bytes'] == 0, final
        assert final['war_channels'] == 1, final
        out.update(final_metrics=final, passed=True)
    except Exception:
        out['error'] = traceback.format_exc()
    finally:
        server.terminate()
        try:
            out['server_exit_code'] = server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait()
            out['server_exit_code'] = 'killed'
path = ROOT / ('artifacts/' + args.prefix + '.json')
path.write_text(json.dumps(out, indent=2) + '\n')
print(json.dumps({k: v for k, v in out.items() if k != 'workers'}))
raise SystemExit(0 if out['passed'] and out['server_exit_code'] == 0 else 1)
