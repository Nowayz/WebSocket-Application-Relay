#!/usr/bin/env python3
"""Exercise real Cargo cache recovery using a tiny dependency-free crate."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

TOOL = Path(__file__).with_name('build-snapshot.py').resolve()


class SnapshotTest(unittest.TestCase):
    def fixture(self, root):
        snapshot = root / 'snapshot'
        (snapshot / 'src').mkdir(parents=True)
        (snapshot / 'Cargo.toml').write_text('[package]\nname="snapshot-fixture"\nversion="0.1.0"\nedition="2024"\n')
        (snapshot / 'Cargo.lock').write_text('version=4\n[[package]]\nname="snapshot-fixture"\nversion="0.1.0"\n')
        (snapshot / 'src/main.rs').write_text('fn main() { println!("diagnostic"); }\n')
        return snapshot

    def invoke(self, root, snapshot, output, kind='bin', name='snapshot-fixture'):
        return subprocess.run([sys.executable, str(TOOL), '--snapshot', str(snapshot),
                               '--target-dir', str(root / 'cache'), '--output', str(output),
                               '--' + kind, name], capture_output=True, text=True, timeout=60)

    def test_stale_source_recompiles_and_existing_frozen_binary_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot = self.fixture(root)
            first = root / 'diagnostic'
            self.assertEqual(self.invoke(root, snapshot, first).returncode, 0)
            source = snapshot / 'src/main.rs'
            source.write_text('fn main() { println!("control"); }\n')
            os.utime(source, (1, 1))
            result = subprocess.run(['cargo', 'build', '--locked', '--release', '--manifest-path',
                                     str(snapshot / 'Cargo.toml'), '--target-dir', str(root / 'cache')],
                                    capture_output=True, text=True, timeout=60)
            self.assertEqual(result.returncode, 0, result.stderr)
            executable = root / 'cache/release/snapshot-fixture'
            self.assertEqual(subprocess.check_output([str(executable)], text=True).strip(), 'diagnostic',
                             'fixture must reproduce the observed stale Cargo cache')
            second = root / 'control'
            result = self.invoke(root, snapshot, second)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(subprocess.check_output([str(second / 'snapshot-fixture')], text=True).strip(), 'control')
            self.assertEqual(subprocess.check_output([str(first / 'snapshot-fixture')], text=True).strip(), 'diagnostic')
            record = json.loads((second / 'build.json').read_text())
            self.assertTrue(record['passed'])
            self.assertFalse(record['cargo_artifact']['fresh'])
            self.assertEqual(record['binary_sha256'], hashlib.sha256((second / 'snapshot-fixture').read_bytes()).hexdigest())
            self.assertEqual(record['source_sha256']['src/main.rs'], hashlib.sha256(source.read_bytes()).hexdigest())
            protected = (second / 'build.json').read_bytes()
            stamp = source.stat().st_mtime_ns
            self.assertEqual(self.invoke(root, snapshot, second).returncode, 2)
            self.assertEqual((second / 'build.json').read_bytes(), protected)
            self.assertEqual(source.stat().st_mtime_ns, stamp)

    def test_example_target_is_selected_from_cargo_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot = self.fixture(root)
            (snapshot / 'examples').mkdir()
            (snapshot / 'examples/probe.rs').write_text('fn main() { println!("example"); }\n')
            output = root / 'example'
            result = self.invoke(root, snapshot, output, kind='example', name='probe')
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(subprocess.check_output([str(output / 'probe')], text=True).strip(), 'example')
            self.assertEqual(json.loads((output / 'build.json').read_text())['cargo_artifact']['target']['kind'], ['example'])

    def test_checkouts_and_symlink_sources_are_rejected_before_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot = self.fixture(root)
            source = snapshot / 'src/main.rs'
            before = source.stat().st_mtime_ns
            (snapshot / '.git').write_text('fixture checkout marker')
            output = root / 'refused'
            self.assertEqual(self.invoke(root, snapshot, output).returncode, 2)
            (snapshot / '.git').unlink()
            (snapshot / 'linked.rs').symlink_to(source)
            self.assertEqual(self.invoke(root, snapshot, output).returncode, 2)
            self.assertEqual(source.stat().st_mtime_ns, before)
            self.assertFalse(output.exists())
            (snapshot / 'linked.rs').unlink()
            output.symlink_to(root / 'missing-output')
            self.assertEqual(self.invoke(root, snapshot, output).returncode, 2)
            self.assertTrue(output.is_symlink())
            self.assertFalse((root / 'missing-output').exists())
            self.assertEqual(source.stat().st_mtime_ns, before)

    def test_build_failure_keeps_a_failed_receipt_without_an_executable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot = self.fixture(root)
            (snapshot / 'src/main.rs').write_text('not Rust source\n')
            output = root / 'failed'
            result = self.invoke(root, snapshot, output)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            record = json.loads((output / 'build.json').read_text())
            self.assertFalse(record['passed'])
            self.assertNotEqual(record['cargo_exit_code'], 0)
            self.assertFalse((output / 'snapshot-fixture').exists())

    def test_source_mutation_during_build_prevents_freezing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            snapshot = self.fixture(root)
            (snapshot / 'build.rs').write_text('fn main() { std::fs::write(std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("changed.txt"), "changed").unwrap(); }\n')
            output = root / 'changed'
            result = self.invoke(root, snapshot, output)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            record = json.loads((output / 'build.json').read_text())
            self.assertFalse(record['passed'])
            self.assertIn('snapshot contents changed', record['error'])
            self.assertFalse((output / 'snapshot-fixture').exists())


if __name__ == '__main__':
    unittest.main()
