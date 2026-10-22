#!/usr/bin/env python3
"""Run vendored rustls unit tests with checksum-pinned upstream test fixtures.

The published crate omits binary test data and the fuzz corpus. Keep those
fixtures in a separate fresh directory; never mutate the vendored package.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def run(directory):
    info = json.loads((ROOT / 'vendor/provenance.json').read_text())['rustls']
    fixture = info['test_fixtures']
    cache = ROOT / 'target/vendor-source-cache'
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / ('rustls-fixtures-' + info['upstream_git'] + '.tar.gz')
    if not archive.exists():
        with urllib.request.urlopen(fixture['url'], timeout=60) as response:
            temporary = archive.with_suffix('.download')
            temporary.write_bytes(response.read())
            temporary.replace(archive)
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    if checksum != fixture['sha256']:
        raise RuntimeError('rustls test-fixture archive checksum mismatch')
    directory = directory.resolve()
    directory.mkdir(parents=True, exist_ok=False)
    with tarfile.open(archive) as contents:
        contents.extractall(directory, filter='data')
    upstream = directory / ('rustls-' + info['upstream_git'])
    project = directory / 'rustls'
    shutil.copytree(ROOT / 'vendor/rustls', project)
    shutil.copytree(upstream / 'rustls/src/testdata', project / 'src/testdata')
    shutil.copytree(upstream / 'test-ca', directory / 'test-ca')
    shutil.copytree(upstream / 'fuzz', directory / 'fuzz')
    command = ['cargo', 'test', '--manifest-path', str(project / 'Cargo.toml'),
               '--locked', '--target-dir', str(ROOT / 'target/rustls-tests'),
               '--no-default-features', '--features', 'ring,std,tls12,logging', '--lib']
    (directory / 'provenance.json').write_text(json.dumps({
        'fixtures': fixture, 'upstream_git': info['upstream_git'],
        'candidate_source_sha256': hashlib.sha256(
            (project / 'src/tls13/key_schedule.rs').read_bytes()).hexdigest(),
        'command': command,
    }, indent=2) + '\n')
    subprocess.run(command, cwd=ROOT, check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True,
                        help='Fresh directory for copied test project and fixtures')
    run(parser.parse_args().directory)
