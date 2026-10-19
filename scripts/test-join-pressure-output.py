#!/usr/bin/env python3
"""Fast artifact-preservation and failed-exec checks for prejoin pressure."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('join_pressure', Path(__file__).with_name('join-pressure.py'))
pressure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pressure)


class OutputTests(unittest.TestCase):
    def args(self, root, server):
        return SimpleNamespace(server=server, output=root / 'result.json', connections=1,
                               waves=1, ping=False, withhold_close_ack=False)

    def test_existing_receipt_or_log_including_dangling_symlinks_is_preserved(self):
        for suffix in ['.json', '.server.log']:
            for dangling in [False, True]:
                with self.subTest(suffix=suffix, dangling=dangling), tempfile.TemporaryDirectory() as folder:
                    root = Path(folder)
                    args = self.args(root, root / 'unused-server')
                    target = args.output.with_suffix(suffix)
                    if dangling:
                        target.symlink_to(root / 'missing')
                    else:
                        target.write_bytes(b'prior evidence\n')
                    with patch.object(pressure.subprocess, 'Popen') as start:
                        with self.assertRaises(FileExistsError):
                            pressure.run(args)
                        start.assert_not_called()
                    if dangling:
                        self.assertTrue(target.is_symlink())
                        self.assertFalse(target.exists())
                    else:
                        self.assertEqual(target.read_bytes(), b'prior evidence\n')
                    other = args.output.with_suffix('.server.log' if suffix == '.json' else '.json')
                    self.assertFalse(os.path.lexists(other))

    @unittest.skipUnless(os.name == 'posix', 'Unix execution errors')
    def test_failed_exec_saves_a_failed_receipt_without_a_server_handle(self):
        for mode in ['missing', 'permission', 'interpreter']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                server = root / 'server'
                if mode == 'permission':
                    server.write_text('#!/bin/sh\nexit 99\n')
                    server.chmod(0o600)
                elif mode == 'interpreter':
                    server.write_text('#!/missing-war-fixture-interpreter\n')
                    server.chmod(0o700)
                args = self.args(root, server)
                with self.assertRaises(OSError):
                    pressure.run(args)
                receipt = json.loads(args.output.read_text())
                self.assertFalse(receipt['passed'])
                self.assertIn('error', receipt)
                self.assertIsNone(receipt['server_exit_code'])
                self.assertNotIn('server_forced_kill', receipt)
                self.assertTrue(args.output.with_suffix('.server.log').is_file())


if __name__ == '__main__':
    unittest.main()
