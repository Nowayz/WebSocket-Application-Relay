#!/usr/bin/env python3
"""Verify a slow destination cannot corrupt healthy traffic or strand resources."""
import importlib.util, json, os, pathlib, subprocess, time
ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
port, admin = (c.wire.runtime.free_port(), c.wire.runtime.free_port())
env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
env['WAR_ADMIN_TOKEN'] = c.TOKEN
cmd = [str(ROOT / 'artifacts/candidates/cpp-modern/war-relay-cpp'), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4', '--max-message-bytes', '1048576', '--queue-bytes', '1048586', '--queue-messages', '64', '--total-queue-bytes', '4194304', '--tcp-send-buffer-bytes', '16384']
record = {'server_command': cmd, 'passed': False}
with open(ROOT / 'artifacts/cpp-modern-pressure-server.log', 'w') as log:
    server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    try:
        for _ in range(200):
            try:
                c.metrics(admin)
                break
            except OSError:
                time.sleep(0.01)
        load = subprocess.run([str(ROOT / 'artifacts/baselines/capacity-final/stress'), '--url', f'ws://127.0.0.1:{port}', '--mode', 'slow', '--duration-secs', '3', '--payload-bytes', '65536'], capture_output=True, text=True, timeout=25)
        record['client_log'] = load.stdout + load.stderr
        record['client_exit_code'] = load.returncode
        assert load.returncode == 0, record['client_log']
        result = next((json.loads(s) for s in load.stdout.splitlines() if s.startswith('{') and json.loads(s).get('event') == 'result'))
        record['result'] = result
        assert result['verified_deliveries'] > 0 and result['slow_observation']['kind'] in ['close', 'eof', 'transport_error'], result
        for _ in range(700):
            final = c.metrics(admin)
            if final['war_connections'] == final['war_queued_bytes'] == final['war_retained_output_bytes'] == 0:
                break
            time.sleep(0.01)
        record['final_metrics'] = final
        assert final['war_connections'] == final['war_users'] == final['war_queued_bytes'] == final['war_buffered_bytes'] == final['war_retained_output_bytes'] == 0, final
        assert final['war_slow_consumers_total'] > 0, final
        record['passed'] = True
    finally:
        server.terminate()
        record['server_exit_code'] = server.wait(timeout=10)
(ROOT / 'artifacts/cpp-modern-pressure.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
