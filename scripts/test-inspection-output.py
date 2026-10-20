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
    def test_inspection_latency_evidence_blocks_launch(self):
        for symlink in [False, True]:
            with self.subTest(symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                output = directory / 'receipt.json'
                latency = directory / 'users.csv'
                if symlink:
                    latency.symlink_to(directory / 'missing.csv')
                else:
                    latency.write_text('previous evidence\n')
                result, marker = self.invoke(directory, output,
                                             extra=['--fanout-latency-output', str(latency)])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(str(latency), result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())
                if symlink:
                    self.assertTrue(latency.is_symlink())
                else:
                    self.assertEqual(latency.read_text(), 'previous evidence\n')

    def test_inspection_latency_paths_cannot_overlap_receipts(self):
        for name in ['receipt.json', 'receipt.json/users.csv', '.']:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                output = directory / 'receipt.json'
                result, marker = self.invoke(directory, output,
                    extra=['--fanout-latency-output', str(directory / name)])
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('output paths overlap', result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())

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

    def test_capacity_latency_and_profile_files_block_launch(self):
        for kind in ['exact', 'windows', 'histogram', 'profile', 'counters']:
            for symlink in [False, True]:
                with self.subTest(kind=kind, symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    output = directory / 'run.json'
                    csv = directory / 'users.csv'
                    if kind in ['exact', 'windows']:
                        extra = ['--user-latency-output', str(csv)]
                        target = csv if kind == 'exact' else csv.with_suffix('.windows.json')
                    elif kind == 'histogram':
                        extra = ['--user-latency-histogram-output', str(csv)]
                        target = csv
                    else:
                        extra = ['--profile'] + (['--profile-counters'] if kind == 'counters' else [])
                        target = output.with_suffix('.perf-stat.csv' if kind == 'counters' else '.perf.data')
                    evidence = b'prior latency or profile evidence\n'
                    absent = directory / 'absent-evidence'
                    if symlink:
                        target.symlink_to(absent)
                    else:
                        target.write_bytes(evidence)
                    result, marker = self.invoke(directory, output, runner='capacity.py', extra=extra)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn(str(target), result.stderr)
                    self.assertFalse(marker.exists())
                    self.assertFalse(output.exists())
                    if symlink:
                        self.assertTrue(target.is_symlink())
                        self.assertFalse(absent.exists())
                    else:
                        self.assertEqual(target.read_bytes(), evidence)

    def test_capacity_output_paths_must_be_distinct_before_launch(self):
        cases = [
            ('run.json', ['--user-latency-output', 'run.json']),
            ('users.windows.json', ['--user-latency-output', 'users.csv']),
            ('users.windows.json', ['--user-latency-output', 'users.']),
            ('.users.windows.json', ['--user-latency-output', '.users']),
            ('users.csv', ['--user-latency-histogram-output', 'users.csv']),
            ('run.json', ['--profile', '--user-latency-output', 'run.perf.data']),
            ('run.json', ['--profile', '--profile-counters', '--user-latency-output', 'run.perf-stat.csv']),
            ('run.json', ['--user-latency-output', 'nested/../run.json']),
        ]
        for name, options in cases:
            with self.subTest(options=options), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                (directory / 'nested').mkdir()
                output = directory / name
                extra = [str(directory / token) if not token.startswith('--') else token for token in options]
                result, marker = self.invoke(directory, output, runner='capacity.py', extra=extra)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn('output paths overlap', result.stderr)
                self.assertFalse(marker.exists())
                self.assertFalse(output.exists())

    def test_server_exec_failures_write_failed_receipts_without_starting_client(self):
        for runner in ['inspection.py', 'capacity.py']:
            for executable in [False, True]:
                with self.subTest(runner=runner, executable=executable), tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary)
                    server = directory / 'server'
                    server.write_text(f'#!{directory / "missing-interpreter"}\n')
                    server.chmod(0o755 if executable else 0o644)
                    marker = directory / 'client-started'
                    client = directory / 'client'
                    client.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                                      f'Path({str(marker)!r}).touch()\n')
                    client.chmod(0o755)
                    output = directory / 'receipt.json'
                    result = subprocess.run(
                        [sys.executable, str(ROOT / 'scripts' / runner),
                         '--server', str(server), '--client', str(client),
                         '--server-cpus', '', '--client-cpus', '', '--output', str(output)],
                        cwd=ROOT, capture_output=True, text=True, timeout=5)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertFalse(marker.exists())
                    receipt = json.loads(output.read_text())
                    self.assertFalse(receipt['passed'])
                    self.assertIn('FileNotFoundError' if executable else 'PermissionError', receipt['error'])
                    self.assertIsNone(receipt.get('server_exit_code'))
                    self.assertEqual(receipt['server_log'], '')
                    self.assertEqual(receipt['client_log'], '')

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
