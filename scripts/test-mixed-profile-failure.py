#!/usr/bin/env python3
"""A profiler exiting successfully by itself must not validate a requested profile."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class ProfileFailureTest(unittest.TestCase):
    def test_profiler_must_survive_until_the_controller_stops_it(self):
        for mode, delay in [("early", 0), ("during", 0.5), ("missing", None), ("empty", None)]:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temp:
                folder = Path(temp)
                marker = folder / 'target.json'
                fake = folder / 'perf'
                program = (f'#!{sys.executable}\nimport json,sys,time,signal\n'
                           f'open({str(marker)!r}, "w").write(json.dumps(int(sys.argv[sys.argv.index("-p")+1])))\n')
                if mode == 'empty':
                    program += 'open(sys.argv[sys.argv.index("-o")+1], "w").close()\n'
                if delay is None:
                    program += 'signal.signal(signal.SIGINT, lambda *_: sys.exit(0))\nwhile True: signal.pause()\n'
                else:
                    program += f'time.sleep({delay})\nraise SystemExit(0)\n'
                fake.write_text(program)
                fake.chmod(0o755)
                output = folder / 'run.json'
                env = os.environ.copy()
                env['PATH'] = str(folder) + os.pathsep + env['PATH']
                command = [sys.executable, str(ROOT / 'scripts/mixed-capacity.py'),
                           '--server', os.environ.get('WAR_MIXED_TEST_SERVER', str(ROOT / 'target/release/war-relay')),
                           '--capacity-client', os.environ.get('WAR_MIXED_TEST_CAPACITY', str(ROOT / 'target/release/examples/capacity')),
                           '--sidecar', os.environ.get('WAR_MIXED_TEST_SIDECAR', str(ROOT / 'target/release/examples/inspection')),
                           '--users', '32', '--seconds', '2', '--readers', '16',
                           '--scope', 'room', '--window', '4', '--inspectors', '0',
                           '--pairs', '0', '--churn-workers', '0', '--fanout-windows-per-second', '1',
                           '--profile', '--profile-target', 'sidecar', '--output', str(output)]
                result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
                receipt = json.loads(output.read_text())
                target = json.loads(marker.read_text())
                with self.assertRaises(ProcessLookupError):
                    os.kill(target, 0)
                print(json.dumps({'mode': mode, 'delay': delay, 'runner_exit': result.returncode,
                                  'passed': receipt['passed'], 'profile': receipt.get('profile'),
                                  'error': receipt.get('error'), 'child_exit_codes': receipt['exit_codes'],
                                  'profile_created': output.with_suffix('.perf.data').exists(),
                                  'target_reaped': True}), flush=True)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertFalse(receipt['passed'])
                self.assertFalse(receipt['correctness_passed'])
                self.assertEqual(receipt['profile']['exit_code'], 0)
                self.assertIn('profiler produced no data' if delay is None else 'profiler exited before', receipt['error'])
                if delay == 0:
                    self.assertFalse(any(e['event'] == 'measured' for e in receipt['events']))


if __name__ == '__main__':
    unittest.main()
