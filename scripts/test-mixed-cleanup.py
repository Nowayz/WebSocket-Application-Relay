#!/usr/bin/env python3
"""A stubborn profiler must not skip mixed-load cleanup or failure receipts."""
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('mixed_cleanup_target', Path(__file__).with_name('mixed-capacity.py'))
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)


class Child:
    def __init__(self, name, events=()):
        self.name = name
        self.pid = 123
        self.returncode = None
        self.stdin = io.StringIO()
        self.stdout = io.StringIO(''.join(json.dumps({'event': e})+'\n' for e in events))
        self.waits = []
        self.killed = self.terminated = False

    def poll(self):
        return self.returncode

    def send_signal(self, _signal):
        pass

    def terminate(self):
        self.terminated = True
        self.returncode = 0

    def kill(self):
        self.killed = True
        self.returncode = -9

    def wait(self, timeout=None):
        self.waits.append(timeout)
        if self.returncode is None:
            raise subprocess.TimeoutExpired(self.name, timeout)
        return self.returncode


class CleanupTest(unittest.TestCase):
    def test_profiler_normal_and_already_exited_paths(self):
        for already_exited in [False, True]:
            with self.subTest(already_exited=already_exited):
                profiler = Mock()
                profiler.returncode = 0
                profiler.poll.return_value = 0 if already_exited else None
                self.assertEqual(mixed.stop_perf(profiler), (0, False))
                profiler.kill.assert_not_called()
                if already_exited:
                    profiler.send_signal.assert_not_called()
                    profiler.wait.assert_not_called()
                else:
                    profiler.send_signal.assert_called_once_with(mixed.signal.SIGINT)
                    profiler.wait.assert_called_once_with(timeout=10)

    def test_profiler_timeout_preserves_cleanup_and_receipt(self):
        for workload_failure in [False, True]:
            with self.subTest(workload_failure=workload_failure), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)/'run.json'
                children = [Child('server'), Child('capacity', ['connected','ready','measured']),
                            Child('sidecar', ['ready']), Child('perf')]
                snapshots = 0
                def snapshot(*_args, **_kwargs):
                    nonlocal snapshots
                    snapshots += 1
                    if workload_failure and snapshots == 2:
                        raise ValueError('controlled workload failure')
                    return {'cpu_seconds': 0, 'fds': 10}
                class Task:
                    name = '123'
                    def __truediv__(self, _name):
                        return self
                    def read_text(self):
                        return 'war-io-0'
                original_iterdir = Path.iterdir
                def iterdir(path):
                    return iter([Task()]) if str(path)=='/proc/123/task' else original_iterdir(path)
                argv = ['mixed-capacity.py','--server','/bin/false','--sidecar','/bin/false',
                        '--capacity-client','/bin/false','--users','32','--readers','16','--seconds','2',
                        '--workers','1','--pairs','0','--inspectors','0','--profile','--output',str(output)]
                with patch.object(sys,'argv',argv), \
                     patch.object(mixed.subprocess,'Popen',side_effect=children), \
                     patch.object(mixed.subprocess,'check_output',return_value='controlled-topology'), \
                     patch.object(mixed.urllib.request,'urlopen',side_effect=lambda *_a,**_k:io.BytesIO(b'war_connections 50\n')), \
                     patch.object(mixed,'snapshot',side_effect=snapshot), \
                     patch.object(Path,'iterdir',iterdir), \
                     patch.object(mixed.os,'sched_getaffinity',return_value={0}), \
                     patch.object(mixed.select,'select',side_effect=lambda reads,*_a:(reads,[],[])), \
                     patch.object(mixed.time,'sleep'):
                    with self.assertRaises(Exception):
                        mixed.main()
                receipt = json.loads(output.read_text())
                self.assertFalse(receipt['passed'])
                self.assertFalse(receipt['correctness_passed'])
                self.assertEqual(receipt['exit_codes'],[0,0,0])
                self.assertTrue(all(c.terminated for c in children[:3]))
                self.assertTrue(children[3].killed)
                self.assertEqual(children[3].waits,[10,5])
                self.assertEqual(receipt['profile']['exit_code'],-9)
                self.assertTrue(receipt['profile']['forced_kill'])
                self.assertIn('controlled workload failure' if workload_failure else 'profiler stop failed',receipt['error'])


if __name__ == '__main__':
    unittest.main()
