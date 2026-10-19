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
    def invoke(self, directory, output, profile=False, runner="inspection.py", extra=()):
        marker = directory / 'server-started'
        server = directory / 'server'
        server.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                          f'Path({str(marker)!r}).touch()\nraise SystemExit(1)\n')
        server.chmod(0o755)
        result = subprocess.run(
            [sys.executable, str(ROOT / 'scripts' / runner),
             '--server', str(server), '--client', '/bin/false',
             '--server-cpus', '', '--client-cpus', '', '--output', str(output)]
            + (['--profile'] if profile else []) + list(extra),
            cwd=ROOT, capture_output=True, text=True, timeout=10)
        return result, marker

    def test_private_payload_must_fit_sequence_marker_before_launch(self):
        for size in [-1, 0, 7]:
            with self.subTest(size=size), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                output = directory / 'receipt.json'
                result, marker = self.invoke(directory, output,
                                             extra=['--private-payload-bytes', str(size)])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('sequence marker', result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())

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

    def test_existing_profile_blocks_fresh_receipt_before_server_start(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            output = directory / 'run.new-extension'
            profile = output.with_suffix('.perf.data')
            evidence = b'previous raw profile evidence'
            profile.write_bytes(evidence)
            result, marker = self.invoke(directory, output, profile=True)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn(str(profile), result.stderr)
            self.assertEqual(profile.read_bytes(), evidence)
            self.assertFalse(output.exists())
            self.assertFalse(marker.exists())

    def test_dangling_profile_symlink_blocks_launch(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            output = directory / 'run.json'
            profile = output.with_suffix('.perf.data')
            target = directory / 'absent-profile'
            profile.symlink_to(target)
            result, marker = self.invoke(directory, output, profile=True)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertTrue(profile.is_symlink())
            self.assertFalse(target.exists())
            self.assertFalse(output.exists())
            self.assertFalse(marker.exists())

    def test_capacity_receipts_and_symlinks_block_launch(self):
        for symlink in [False, True]:
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                output = directory / 'capacity.json'
                target = directory / 'absent-target'
                evidence = b'previous capacity evidence\n'
                if symlink:
                    output.symlink_to(target)
                else:
                    output.write_bytes(evidence)
                result, marker = self.invoke(directory, output, runner="capacity.py")
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(str(output), result.stderr)
                self.assertFalse(marker.exists())
                if symlink:
                    self.assertTrue(output.is_symlink())
                    self.assertFalse(target.exists())
                else:
                    self.assertEqual(output.read_bytes(), evidence)

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
