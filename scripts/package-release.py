#!/usr/bin/env python3
"""Build traceable local release assets from a clean committed source tree.

This builds and packages; it does not tag, publish or install a service.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def sha256(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def pack_tree(root, destination, epoch):
    def normalize(info):
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        info.mtime = epoch
        return info

    with destination.open('xb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode='w') as archive:
            archive.add(root, arcname=root.name, filter=normalize)


def dependency_notices(build_log, destination, source):
    """Include notices for the exact compiled dependency set, including build tools."""
    packages = {}
    for line in build_log.read_text().splitlines():
        try:
            artifact = json.loads(line)
        except ValueError:
            continue
        if artifact.get('reason') != 'compiler-artifact':
            continue
        manifest = Path(artifact['manifest_path'])
        if manifest == source / 'Cargo.toml':
            continue
        packages[artifact['package_id']] = manifest
    index = []
    for identity, manifest in sorted(packages.items()):
        package = tomllib.loads(manifest.read_text())['package']
        name = f"{package['name']}-{package['version']}"
        notices = sorted(p for p in manifest.parent.iterdir()
                         if p.is_file() and p.name.startswith(('LICENSE', 'COPYING', 'NOTICE')))
        if not notices:
            raise ValueError(f'No dependency notices found for {identity}')
        directory = destination / name
        directory.mkdir(parents=True)
        for notice in notices:
            shutil.copy2(notice, directory / notice.name)
        index.append({'package': identity, 'directory': name, 'license': package.get('license'),
                      'files': {p.name: sha256(p) for p in notices}})
    (destination / 'index.json').write_text(json.dumps(index, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New directory; never overwritten')
    parser.add_argument('--target-dir', type=Path, default=ROOT / 'target', help='Cargo cache; serialize builds')
    args = parser.parse_args()
    if subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=normal'], cwd=ROOT):
        parser.error('commit or preserve outstanding changes before packaging a release')
    output, cache = args.output.resolve(), args.target_dir.resolve()
    if output == cache or output in cache.parents or cache in output.parents:
        parser.error('output and Cargo cache must not contain one another')
    try:
        args.output.mkdir(parents=True, exist_ok=False)
    except OSError as error:
        parser.error(str(error))
    record = {'status': 'failed', 'assets': {}}
    # Release code uses the compiler baseline, not a caller's local CPU tuning or cross target.
    env = {k: v for k, v in os.environ.items()
           if k not in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_TARGET')}
    try:
        record['commit'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
        epoch = int(subprocess.check_output(['git', 'show', '-s', '--format=%ct', 'HEAD'], cwd=ROOT))
        record['tools'] = {name: subprocess.check_output(command, text=True).strip() for name, command in (
            ('rustc', ['rustc', '-vV']), ('cargo', ['cargo', '--version']),
            ('node', ['node', '--version']), ('npm', ['npm', '--version']))}
        host = next(line[6:] for line in record['tools']['rustc'].splitlines() if line.startswith('host: '))
        record['host'] = host
        with tempfile.TemporaryDirectory(prefix='war-release-') as temporary:
            work = Path(temporary)
            source_tar = work / 'source.tar'
            with source_tar.open('xb') as stream:
                subprocess.run(['git', 'archive', '--format=tar', 'HEAD'], cwd=ROOT, stdout=stream, check=True)
            source = work / 'source'
            source.mkdir()
            with tarfile.open(source_tar) as archive:
                archive.extractall(source, filter='data')
            version = tomllib.loads((source / 'Cargo.toml').read_text())['package']['version']
            package = json.loads((source / 'clients/js/package.json').read_text())
            if package['version'] != version:
                raise ValueError('Rust and JavaScript release versions must agree')
            record['version'] = version
            source_name = f'war-relay-{version}-source'
            source.rename(work / source_name)
            source = work / source_name
            pack_tree(source, output / f'{source_name}.tar.gz', epoch)
            with (output / 'package.log').open('x') as log:
                def run(command, cwd=source):
                    subprocess.run(command, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)

                run([sys.executable, str(source / 'scripts/build-snapshot.py'), '--snapshot', str(source),
                     '--target-dir', str(cache), '--output', str(output / 'build'), '--bin', 'war-relay'])
                client = source / 'clients/js'
                run(['npm', 'ci', '--omit=optional', '--ignore-scripts', '--no-audit', '--no-fund'], client)
                run(['npm', 'run', 'check:types'], client)
                run(['npm', 'run', 'build:browser'], client)
                # npm always includes LICENSE; use the project's authoritative license.
                shutil.copy2(source / 'LICENSE', client / 'LICENSE')
                run(['npm', 'pack', '--ignore-scripts', '--pack-destination', str(output)], client)
            runtime = work / f'war-relay-{version}-{host}'
            (runtime / 'bin').mkdir(parents=True)
            shutil.copy2(output / 'build/war-relay', runtime / 'bin/war-relay')
            dependency_notices(output / 'build/build.log', runtime / 'third-party', source)
            for name in ('docs', 'deploy'):
                shutil.copytree(source / name, runtime / name)
            shutil.copytree(source / 'target/browser', runtime / 'browser')
            shutil.copy2(source / 'LICENSE', runtime / 'LICENSE')
            shutil.copy2(source / 'deploy/README.md', runtime / 'README.md')
            (runtime / 'release.json').write_text(json.dumps({k: record[k] for k in ('commit', 'version', 'host', 'tools')}, indent=2) + '\n')
            pack_tree(runtime, output / f'{runtime.name}.tar.gz', epoch)
        record['assets'] = {p.name: sha256(p) for p in sorted(output.glob('*.tgz')) + sorted(output.glob('*.tar.gz'))}
        if len(record['assets']) != 3:
            raise ValueError('expected exactly server, source and Node client archives')
        (output / 'SHA256SUMS').write_text(''.join(f'{digest}  {name}\n' for name, digest in record['assets'].items()))
        record['status'] = 'built'
    except Exception as error:
        record['error'] = str(error)
    (output / 'release.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return 0 if record['status'] == 'built' else 1


if __name__ == '__main__':
    raise SystemExit(main())
