#!/usr/bin/env python3
"""A profiler timeout must not bypass other child cleanup or receipt writing."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('inspection_cleanup_target', ROOT / 'scripts/inspection.py')
inspection = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inspection)


class Child:
    def __init__(self, name, output=''):
        self.name = name
        self.pid = 123
        self.returncode = None
        self.stdin = io.StringIO()
        self.stdout = io.StringIO(output)
        self.killed = self.terminated = False
        self.waits = []

    def poll(self):
        return self.returncode

    def send_signal(self, _signal):
        pass  # The controlled profiler ignores SIGINT.

    def kill(self):
        self.killed = True
        self.returncode = -9

    def terminate(self):
        self.terminated = True
        self.returncode = 0

    def wait(self, timeout=None):
        self.waits.append(timeout)
        if self.returncode is None:
            raise subprocess.TimeoutExpired(self.name, timeout)
        return self.returncode


class CleanupTest(unittest.TestCase):
    def test_profiler_timeout_retires_other_children_and_saves_failure(self):
        for workload_failure in [False, True]:
            with self.subTest(workload_failure=workload_failure), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary) / 'receipt.json'
                args = SimpleNamespace(
                    output=output, profile=True, server=Path('/bin/false'), client=Path('/bin/false'),
                    heap_profile=None, workers=4, mode='none', rooms=0, seconds=1,
                    inspectors=0, inspection_interval_ms=10, pairs=1, churn_workers=0,
                    fanout_readers=0, fanout_window=16, server_cpus='', client_cpus='',
                    read_storage='worker',
                )
                events = [
                    {'event': 'ready', 'details': {'stable_sessions': 2, 'stable_channels': 1}},
                    {'event': 'measured', 'details': {'elapsed_seconds': 1}},
                ]
                server = Child('server')
                client = Child('client', ''.join(json.dumps(event) + '\n' for event in events))
                profiler = Child('perf')
                children = iter([server, client, profiler])

                def launch(_command, **kwargs):
                    child = next(children)
                    if child is profiler:
                        kwargs['stdout'].write(b'controlled profiler output\n')
                    return child

                checks = [None, ValueError('controlled workload failure')] if workload_failure else None
                with patch.object(inspection.platform, 'platform', return_value='controlled-host'), \
                     patch.object(inspection.subprocess, 'Popen', side_effect=launch), \
                     patch.object(inspection, 'metrics', return_value={}), \
                     patch.object(inspection, 'check', side_effect=checks), \
                     patch.object(inspection.runtime, 'snapshot', return_value={'cpu_seconds': 0}), \
                     patch.object(inspection.select, 'select', return_value=([client.stdout], [], [])), \
                     patch.object(inspection.time, 'sleep'):
                    result = inspection.run(args)
                self.assertFalse(result['passed'])
                self.assertTrue(profiler.killed)
                self.assertEqual(profiler.waits, [10, 5], 'one grace wait, then bounded reap')
                self.assertTrue(client.killed)
                self.assertTrue(server.terminated)
                self.assertTrue(client.stdin.closed and client.stdout.closed)
                self.assertEqual(result['perf_exit_code'], -9)
                self.assertTrue(result['perf_forced_kill'])
                self.assertEqual(result['server_exit_code'], 0)
                self.assertEqual(result['client_exit_code'], -9)
                self.assertIn('controlled profiler output', result['perf_log'])
                if workload_failure:
                    self.assertIn('controlled workload failure', result['error'])
                self.assertEqual(json.loads(output.read_text()), result)


if __name__ == '__main__':
    unittest.main()
