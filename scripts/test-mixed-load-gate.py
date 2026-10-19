#!/usr/bin/env python3
"""Real short mixed workloads distinguish correctness from meeting offered load.

Build war-relay, capacity and inspection in release mode first, or supply the
WAR_MIXED_TEST_SERVER, WAR_MIXED_TEST_CAPACITY and WAR_MIXED_TEST_SIDECAR paths.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class OfferedLoadTest(unittest.TestCase):
    def test_paced_success_overload_and_unpaced_success(self):
        for rate, expected in [(1, True), (1_000_000, False), (0, True)]:
            with self.subTest(rate=rate), tempfile.TemporaryDirectory() as temp:
                output = Path(temp) / 'run.json'
                command = [sys.executable, str(ROOT / 'scripts/mixed-capacity.py'),
                           '--server', os.environ.get('WAR_MIXED_TEST_SERVER', str(ROOT / 'target/release/war-relay')),
                           '--capacity-client', os.environ.get('WAR_MIXED_TEST_CAPACITY', str(ROOT / 'target/release/examples/capacity')),
                           '--sidecar', os.environ.get('WAR_MIXED_TEST_SIDECAR', str(ROOT / 'target/release/examples/inspection')),
                           '--users', '32', '--seconds', '2', '--readers', '16',
                           '--scope', 'room', '--window', '4', '--inspectors', '0',
                           '--pairs', '0', '--churn-workers', '0', '--churn-cycles-per-second', '0',
                           '--fanout-windows-per-second', str(rate), '--pin-client-workers',
                           '--output', str(output)]
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=45)
                self.assertEqual(result.returncode, 0 if expected else 1, result.stdout + result.stderr)
                receipt = json.loads(output.read_text())
                self.assertTrue(receipt['correctness_passed'])
                self.assertEqual(receipt['args']['churn_workers'], 0)
                ready = next(e for e in receipt['sidecar_events'] if e['event'] == 'ready')
                measured = next(e for e in receipt['sidecar_events'] if e['event'] == 'measured')
                self.assertEqual(receipt['sidecar_events'][-1]['details']['counts']['joins'], ready['details']['counts']['joins'])
                self.assertEqual(measured['details']['churn']['verified'], 0)
                self.assertEqual(receipt['passed'], expected)
                self.assertEqual(receipt['exit_codes'], [0, 0, 0])
                self.assertEqual(receipt['fanout_offered_load_met'], None if rate == 0 else expected)
                summary = json.loads(result.stdout.splitlines()[-1])
                self.assertEqual(summary['passed'], expected)
                self.assertTrue(summary['correctness_passed'])
                print(json.dumps({'rate': rate, 'exit_code': result.returncode,
                                  'passed': receipt['passed'], 'correctness_passed': receipt['correctness_passed'],
                                  'fanout_windows': receipt['fanout_windows']}), flush=True)


if __name__ == '__main__':
    unittest.main()
