#!/usr/bin/env python3
"""Exercise recovery from a suppressed dump signal against the real profiler."""
import importlib.util
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('capacity_heap', Path(__file__).with_name('capacity-heap.py'))
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)
LIBRARY = Path('/usr/lib/libtcmalloc.so')


@unittest.skipUnless(sys.platform.startswith('linux') and LIBRARY.is_file(), 'requires Linux gperftools')
class HeapCapture(unittest.TestCase):
    def test_ignored_request_recovers_and_following_gate_is_distinct(self):
        with tempfile.TemporaryDirectory(prefix='war-heap-signal-test-') as directory:
            prefix = Path(directory) / 'relay'
            environment = dict(os.environ, LD_PRELOAD=str(LIBRARY), HEAPPROFILE=str(prefix),
                               HEAPPROFILESIGNAL=str(signal.SIGUSR2),
                               HEAP_PROFILE_ALLOCATION_INTERVAL='0', HEAP_PROFILE_INUSE_INTERVAL='0',
                               HEAP_PROFILE_DEALLOCATION_INTERVAL='0', HEAP_PROFILE_TIME_INTERVAL='0')
            with tempfile.TemporaryFile() as errors:
                child = subprocess.Popen([sys.executable, '-c',
                    'import sys; payload=bytearray(1024*1024); print("ready",flush=True); sys.stdin.readline()'],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, text=True, env=environment)
                original_kill = os.kill
                suppressed = False

                def send(pid, number):
                    nonlocal suppressed
                    if pid == child.pid and number == signal.SIGUSR2 and not suppressed:
                        suppressed = True  # Emulate the profiler's documented TryLock refusal.
                        return
                    original_kill(pid, number)

                try:
                    self.assertTrue(select.select([child.stdout], [], [], 10)[0])
                    self.assertEqual(child.stdout.readline(), 'ready\n')
                    os.kill = send
                    first = collector.capture(child.pid, prefix, 5)
                    self.assertGreaterEqual(first['signal_attempts'], 2)
                    os.kill = original_kill
                    second = collector.capture(child.pid, prefix, 5)
                    self.assertNotEqual(first['path'], second['path'])
                    for profile in (first, second):
                        self.assertIn('heap profile:', profile['header'])
                        self.assertIn(b'MAPPED_LIBRARIES:', Path(profile['path']).read_bytes())
                        self.assertTrue(profile['gate_profiles'])
                    self.assertIsNone(child.poll())
                finally:
                    os.kill = original_kill
                    child.stdin.write('\n'); child.stdin.flush()
                    child.communicate(timeout=10)
                self.assertEqual(child.returncode, 0)


if __name__ == '__main__':
    unittest.main()
