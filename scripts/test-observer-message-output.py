#!/usr/bin/env python3
"""Evidence preservation and failed-startup checks for the observer message screen."""
from pathlib import Path
import importlib.util
import json
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReceiptTest(unittest.TestCase):
    def test_routed_packet_headers_cover_all_integer_width_boundaries(self):
        spec = importlib.util.spec_from_file_location(
            'observer_messages', ROOT / 'scripts/observer-message-fairness.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        headers = {0: '00', 23: '17', 24: '1818', 255: '18ff', 256: '190100',
                   65535: '19ffff', 65536: '1a00010000', 2**32 - 1: '1affffffff',
                   2**32: '1b0000000100000000', 2**64 - 1: '1bffffffffffffffff'}
        for identifier, encoded in headers.items():
            with self.subTest(identifier=identifier):
                self.assertEqual(module.packet(identifier, b'\xf5'),
                                 b'\x82' + bytes.fromhex(encoded) + b'\xf5')

    def invoke(self, directory, extra=()):
        marker = directory / 'server-started'
        server = directory / 'server'
        server.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                          f'Path({str(marker)!r}).touch()\nraise SystemExit(1)\n')
        server.chmod(0o755)
        output = directory / 'receipt.json'
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/observer-message-fairness.py'),
                                 '--server', str(server), '--output', str(output), *extra],
                                cwd=ROOT, capture_output=True, text=True, timeout=10)
        return result, marker, output

    def test_existing_receipt_or_dangling_symlink_blocks_server_launch(self):
        for symlink in [False, True]:
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                output = directory / 'receipt.json'
                if symlink:
                    output.symlink_to(directory / 'missing.json')
                else:
                    output.write_bytes(b'previous evidence\n')
                result, marker, _ = self.invoke(directory)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(marker.exists())
                if symlink:
                    self.assertTrue(output.is_symlink())
                    self.assertFalse((directory / 'missing.json').exists())
                else:
                    self.assertEqual(output.read_bytes(), b'previous evidence\n')

    def test_profile_collision_or_dangling_symlink_blocks_server_launch(self):
        for symlink in [False, True]:
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                profile = directory / 'receipt.perf.data'
                if symlink:
                    profile.symlink_to(directory / 'missing.data')
                else:
                    profile.write_bytes(b'previous profile\n')
                result, marker, output = self.invoke(directory, ['--profile'])
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())
                if symlink:
                    self.assertTrue(profile.is_symlink())
                    self.assertFalse((directory / 'missing.data').exists())
                else:
                    self.assertEqual(profile.read_bytes(), b'previous profile\n')

    def test_invalid_resource_arguments_fail_before_launch(self):
        for flag in ['--audience', '--workers', '--rounds', '--queue-messages', '--publishers']:
            with self.subTest(flag=flag), tempfile.TemporaryDirectory() as temporary:
                result, marker, output = self.invoke(Path(temporary), [flag, '0'])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())

    def test_failed_startup_is_recorded_and_reaped(self):
        with tempfile.TemporaryDirectory() as temporary:
            result, marker, output = self.invoke(Path(temporary))
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertTrue(marker.exists())
            receipt = json.loads(output.read_text())
            self.assertFalse(receipt['passed'])
            self.assertEqual(receipt['server_exit'], 1)
            self.assertIn('server startup failed', receipt['error'])
            self.assertFalse(Path('/proc', str(receipt['server_pid'])).exists())

    def test_combined_scope_and_byte_budget_are_validated_before_launch(self):
        for extra in [['--room-readers', '-1'], ['--room-readers', '1', '--kind', 'private'],
                      ['--queue-bytes', '1046'], ['--room-readers', '1', '--publishers', '2'],
                      ['--kind', 'mixed'], ['--payload-bytes', '7'], ['--payload-bytes', '65523']]:
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as temporary:
                result, marker, output = self.invoke(Path(temporary), extra)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
