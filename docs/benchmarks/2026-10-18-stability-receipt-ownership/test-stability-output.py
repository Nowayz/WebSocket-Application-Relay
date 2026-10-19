#!/usr/bin/env python3
"""Stability runs own one fresh receipt and preserve it on workload failure."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import stability

ROOT = Path(__file__).resolve().parent.parent


class StabilityOutputTest(unittest.TestCase):
    def setup_fixture(self, folder):
        marker = folder / 'pid'
        server = folder / 'server'
        server.write_text(f'#!{sys.executable}\nimport os,sys\nfrom pathlib import Path\n'
                          f'Path({str(marker)!r}).write_text(str(os.getpid()))\n'
                          'sys.stderr.write("controlled startup failure\\n")\nraise SystemExit(7)\n')
        server.chmod(0o755)
        return marker, [sys.executable, str(ROOT/'scripts/stability.py'), '--server', str(server),
                        '--stress', '/bin/false', '--modes', 'traffic', '--traffic-secs', '1', '--clients', '4']

    def test_existing_regular_or_dangling_receipt_blocks_server_start(self):
        for dangling in [False, True]:
            with self.subTest(dangling=dangling), tempfile.TemporaryDirectory() as temp:
                folder = Path(temp)
                marker, command = self.setup_fixture(folder)
                output = folder/'run.json'
                absent = folder/'absent'
                evidence = b'previous measured evidence\n'
                if dangling: output.symlink_to(absent)
                else: output.write_bytes(evidence)
                result = subprocess.run(command+['--output',str(output)], capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('output already exists', result.stderr)
                self.assertFalse(marker.exists())
                if dangling:
                    self.assertTrue(output.is_symlink())
                    self.assertFalse(absent.exists())
                else: self.assertEqual(output.read_bytes(), evidence)

    def test_receipt_created_during_metadata_collection_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            marker, command = self.setup_fixture(folder)
            output = folder/'run.json'
            evidence = 'concurrent evidence\n'
            git = folder/'git'
            git.write_text(f'#!{sys.executable}\nimport sys\nfrom pathlib import Path\n'
                           f'p=Path({str(output)!r})\n'
                           f'if not p.exists(): p.write_text({evidence!r})\n'
                           'if "rev-parse" in sys.argv: print("fixture-revision")\n')
            git.chmod(0o755)
            env = os.environ.copy(); env['PATH'] = str(folder)+os.pathsep+env['PATH']
            result = subprocess.run(command+['--output',str(output)], env=env, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn('output already exists', result.stderr)
            self.assertEqual(output.read_text(), evidence)
            self.assertFalse(marker.exists())

    def test_workload_failure_leaves_parseable_metadata_and_original_error(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            marker, command = self.setup_fixture(folder)
            output = folder/'run.json'
            result = subprocess.run(command+['--output',str(output)], capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            receipt = json.loads(output.read_text())
            self.assertEqual(receipt['cases'], [])
            self.assertIn('controlled startup failure', receipt['error'])
            self.assertIn('server_sha256', receipt)
            with self.assertRaises(ProcessLookupError): os.kill(int(marker.read_text()), 0)


    def test_later_failure_preserves_an_already_completed_case(self):
        with tempfile.TemporaryDirectory() as temp:
            output = Path(temp)/'run.json'
            first = {'mode': 'traffic', 'passed': True, 'verified': 42}
            args = ['stability.py', '--server', '/bin/true', '--stress', '/bin/false',
                    '--modes', 'traffic', 'churn', '--output', str(output)]
            with patch.object(sys, 'argv', args), patch.object(stability, 'run_case',
                    side_effect=[first, RuntimeError('controlled later failure')]) as run:
                with self.assertRaisesRegex(RuntimeError, 'controlled later failure'):
                    stability.main()
            receipt = json.loads(output.read_text())
            self.assertEqual(receipt['cases'], [first])
            self.assertIn('controlled later failure', receipt['error'])
            self.assertEqual(run.call_count, 2)


if __name__ == '__main__':
    unittest.main()
