#!/usr/bin/env python3
"""Timeline collisions fail before optional plotting imports or input reads."""
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().with_name('plot-latency-timeline.py')


class TimelineOutputTest(unittest.TestCase):
    def test_existing_outputs_are_preserved_before_input_or_plotting(self):
        for suffix in ['.png', '.pdf', '.json']:
            for symlink in [False, True]:
                with self.subTest(suffix=suffix, symlink=symlink), tempfile.TemporaryDirectory() as directory:
                    stem = Path(directory) / 'plot.old'
                    output = stem.with_suffix(suffix)
                    if symlink:
                        output.symlink_to('missing-target')
                    else:
                        output.write_bytes(b'prior evidence\n')
                    run = subprocess.run(
                        [sys.executable, '-S', str(SCRIPT), '--case', 'sample=missing-input.json',
                         '--output', str(stem)], capture_output=True, text=True, timeout=3)
                    self.assertEqual(run.returncode, 2, run.stderr)
                    self.assertIn(f'output already exists: {output}', run.stderr)
                    self.assertNotIn('Traceback', run.stderr)
                    if symlink:
                        self.assertTrue(output.is_symlink())
                        self.assertEqual(str(output.readlink()), 'missing-target')
                    else:
                        self.assertEqual(output.read_bytes(), b'prior evidence\n')
                    self.assertEqual(list(Path(directory).iterdir()), [output])


if __name__ == '__main__':
    unittest.main()
