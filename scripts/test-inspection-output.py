#!/usr/bin/env python3
"""A rerun must preserve earlier evidence and never start its relay."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class ReceiptTest(unittest.TestCase):
    def invoke(self, directory, output):
        marker = directory / 'server-started'
        server = directory / 'server'
        server.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                          f'Path({str(marker)!r}).touch()\nraise SystemExit(1)\n')
        server.chmod(0o755)
        result = subprocess.run(
            [sys.executable, str(ROOT / 'scripts/inspection.py'),
             '--server', str(server), '--client', '/bin/false',
             '--server-cpus', '', '--client-cpus', '', '--output', str(output)],
            cwd=ROOT, capture_output=True, text=True, timeout=10)
        return result, marker

    def test_existing_receipt_is_preserved_without_starting_server(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            output = directory / 'receipt.json'
            evidence = b'{"passed": true, "previous": "measured evidence"}\n'
            output.write_bytes(evidence)
            result, marker = self.invoke(directory, output)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn('output already exists', result.stderr)
            self.assertEqual(output.read_bytes(), evidence)
            self.assertFalse(marker.exists())

    def test_dangling_symlink_is_not_followed_or_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            target = directory / 'absent-evidence.json'
            output = directory / 'receipt.json'
            output.symlink_to(target)
            result, marker = self.invoke(directory, output)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertTrue(output.is_symlink())
            self.assertFalse(target.exists())
            self.assertFalse(marker.exists())

    def test_fresh_path_records_startup_failure(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            output = directory / 'new-directory/receipt.json'
            result, marker = self.invoke(directory, output)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertTrue(marker.exists())
            receipt = json.loads(output.read_text())
            self.assertFalse(receipt['passed'])
            self.assertEqual(receipt['server_exit_code'], 1)
            self.assertFalse(receipt['server_forced_kill'])
            self.assertIn('readiness', receipt['error'])


if __name__ == '__main__':
    unittest.main()
