#!/usr/bin/env python3
"""Mixed-load companion evidence must block a reused artifact prefix."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class MixedOutputTest(unittest.TestCase):
    def test_artifact_collisions_are_rejected_before_process_start(self):
        for suffix in ['.json', '.csv', '.windows.json', '-server.err', '-capacity.err',
                       '-sidecar.err', '-fanout.csv', '.perf.data', '-perf.log']:
            for dangling in [False, True]:
                with self.subTest(suffix=suffix, dangling=dangling), tempfile.TemporaryDirectory() as temp:
                    root = Path(temp)
                    output = root / 'run.json'
                    artifact = root / ('run' + suffix)
                    absent = root / 'absent-target'
                    marker = root / 'started'
                    server = root / 'server'
                    server.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                                      f'Path({str(marker)!r}).touch()\nraise SystemExit(1)\n')
                    server.chmod(0o755)
                    evidence = b'previous benchmark evidence\n'
                    if dangling:
                        artifact.symlink_to(absent)
                    else:
                        artifact.write_bytes(evidence)
                    result = subprocess.run(
                        [sys.executable, str(ROOT / 'scripts/mixed-capacity.py'),
                         '--server', str(server), '--capacity-client', '/bin/false',
                         '--sidecar', '/bin/false', '--exact-fanout-latency', '--profile',
                         '--output', str(output)], cwd=ROOT, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 2, result.stderr)
                    self.assertIn(str(artifact), result.stderr)
                    self.assertFalse(marker.exists())
                    if dangling:
                        self.assertTrue(artifact.is_symlink())
                        self.assertFalse(absent.exists())
                    else:
                        self.assertEqual(artifact.read_bytes(), evidence)
                    if suffix != '.json':
                        self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
