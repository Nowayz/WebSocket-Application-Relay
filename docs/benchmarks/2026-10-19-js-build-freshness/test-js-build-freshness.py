#!/usr/bin/env python3
"""Check source fidelity before the JS fixture starts any service."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / 'scripts/check-js.sh'


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


def tree(root, label, dependency):
    (root / 'src/bin').mkdir(parents=True)
    (root / 'vendor/marker/src').mkdir(parents=True)
    (root / 'scripts').mkdir()
    shutil.copyfile(CHECKER, root / 'scripts/check-js.sh')
    (root / 'Cargo.toml').write_text('[package]\nname="war-relay"\nversion="0.1.0"\nedition="2024"\n'
                                      '[dependencies]\nmarker={path="vendor/marker"}\n')
    (root / 'vendor/marker/Cargo.toml').write_text('[package]\nname="marker"\nversion="0.1.0"\nedition="2024"\n')
    (root / 'vendor/marker/src/lib.rs').write_text(f'pub const MARKER: &str = "{dependency}";\n')
    (root / 'src/lib.rs').write_text(f'pub fn marker() -> String {{ format!("{label}-{{}}", marker::MARKER) }}\n')
    for name in ['war-relay', 'war-legacy-bridge']:
        (root / f'src/bin/{name}.rs').write_text(
            'fn main() {\n'
            ' let root = std::env::var("CHECK_JS_CACHE_MARKERS").unwrap();\n'
            f' std::fs::write(std::path::Path::new(&root).join("{name}"), war_relay::marker()).unwrap();\n'
            ' std::thread::sleep(std::time::Duration::from_millis(50));\n'
            ' std::process::exit(1);\n}\n')


def environment(root):
    commands = root / 'commands'
    commands.mkdir()
    executable(commands / 'npm', '#!/bin/sh\nexit 0\n')
    executable(commands / 'curl', '#!/bin/sh\nexit 1\n')
    markers = root / 'markers'
    markers.mkdir()
    env = os.environ.copy()
    env.update(PATH=str(commands) + os.pathsep + env['PATH'],
               CARGO_TARGET_DIR=str(root / 'cache'), CARGO_NET_OFFLINE='true',
               CHECK_JS_CACHE_MARKERS=str(markers))
    return env, commands, markers


class BuildFreshnessTests(unittest.TestCase):
    def test_shared_cache_builds_current_root_and_vendored_sources(self):
        for first_label, second_label in [('A', 'B'), ('same', 'same')]:
            with self.subTest(root_label=second_label), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                env, _, markers = environment(root)
                first, second = root / 'first', root / 'second'
                tree(first, first_label, 'A')
                tree(second, second_label, 'B')
                build = subprocess.run(['cargo', 'build', '--bins'], cwd=first, env=env,
                                       capture_output=True, text=True, timeout=30)
                self.assertEqual(build.returncode, 0, build.stderr)
                shutil.copyfile(first / 'Cargo.lock', second / 'Cargo.lock')
                for path in second.rglob('*'):
                    if path.is_file():
                        os.utime(path, (1, 1))
                done = subprocess.run(['bash', 'scripts/check-js.sh'], cwd=second, env=env,
                                      capture_output=True, text=True, timeout=30)
                # These intentionally failing binaries never open a listener.
                self.assertNotEqual(done.returncode, 0)
                observed = [p.read_text() for p in markers.iterdir()]
                self.assertTrue(observed, done.stdout + done.stderr)
                self.assertEqual(set(observed), {second_label + '-B'}, done.stdout + done.stderr)

    def test_fresh_or_foreign_artifacts_are_rejected_before_launch(self):
        for foreign in [False, True]:
            with self.subTest(foreign=foreign), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                env, commands, markers = environment(root)
                source = root / 'source'
                tree(source, 'unused', 'unused')
                (source / 'Cargo.lock').write_text('# fixture\n')
                entries = []
                for name in ['war-relay', 'war-legacy-bridge']:
                    binary = root / name
                    executable(binary, '#!/bin/sh\nprintf started >"$CHECK_JS_CACHE_MARKERS/' + name + '"\nexit 1\n')
                    entries.append({'reason': 'compiler-artifact', 'target': {'name': name, 'kind': ['bin']},
                                    'manifest_path': str(root / 'foreign/Cargo.toml' if foreign else source / 'Cargo.toml'),
                                    'executable': str(binary), 'fresh': not foreign})
                fixture = root / 'artifacts.jsonl'
                fixture.write_text(''.join(json.dumps(row) + '\n' for row in entries))
                executable(commands / 'cargo', '#!/bin/sh\ncat "' + str(fixture) + '"\n')
                done = subprocess.run(['bash', 'scripts/check-js.sh'], cwd=source, env=env,
                                      capture_output=True, text=True, timeout=10)
                self.assertNotEqual(done.returncode, 0)
                self.assertEqual(list(markers.iterdir()), [], done.stdout + done.stderr)


if __name__ == '__main__':
    unittest.main()
