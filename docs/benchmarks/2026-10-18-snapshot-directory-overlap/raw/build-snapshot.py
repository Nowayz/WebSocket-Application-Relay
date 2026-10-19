#!/usr/bin/env python3
"""Recompile a copied Cargo snapshot and freeze one release executable with evidence.

Run builds serially, including other Cargo users of the shared target directory.
This records snapshot contents and the frozen executable, not external dependency
contents or the complete compiler environment.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def sources(root):
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink() or path.name == '.git':
            raise ValueError('snapshot must contain no symlinks or Git metadata')
        if path.is_file():
            result[str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--snapshot', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New directory; never reused')
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument('--bin')
    target.add_argument('--example')
    args = parser.parse_args()
    root, cache, output = args.snapshot.resolve(), args.target_dir.resolve(), args.output.resolve()
    try:
        if (root / '.git').exists() or (root / '.git').is_symlink():
            raise ValueError('use a copied snapshot, not a Git checkout')
        if not (root / 'Cargo.toml').is_file() or not (root / 'Cargo.lock').is_file():
            raise ValueError('snapshot needs Cargo.toml and Cargo.lock')
        if root == cache or root in cache.parents or cache in root.parents:
            raise ValueError('snapshot and target directories must not contain one another')
        if root == output or root in output.parents or output in root.parents:
            raise ValueError('snapshot and output directories must not contain one another')
        if cache == output or cache in output.parents or output in cache.parents:
            raise ValueError('target and output directories must be separate')
        before = sources(root)
        # Use the original output spelling so a dangling symlink also refuses reuse.
        args.output.mkdir(parents=True, exist_ok=False)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    kind, name = ('bin', args.bin) if args.bin is not None else ('example', args.example)
    command = ['cargo', 'build', '--locked', '--release', '--manifest-path', str(root / 'Cargo.toml'),
               '--target-dir', str(cache), '--message-format=json-render-diagnostics', '--' + kind, name]
    record = {'passed': False, 'snapshot': str(root), 'command': command, 'source_sha256': before}
    try:
        for relative in before:
            (root / relative).touch()
        with (output / 'build.log').open('x') as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        record['cargo_exit_code'] = result.returncode
        if result.returncode != 0:
            raise ValueError('Cargo failed; see build.log')
        artifacts = []
        for line in (output / 'build.log').read_text().splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(message, dict):
                continue
            if (message.get('reason') == 'compiler-artifact'
                    and message.get('manifest_path') == str(root / 'Cargo.toml')
                    and message['target']['name'] == name and kind in message['target']['kind']
                    and message.get('executable')):
                artifacts.append(message)
        if len(artifacts) != 1 or artifacts[0].get('fresh') is not False:
            raise ValueError('Cargo did not confirm recompilation of exactly one requested executable')
        if sources(root) != before:
            raise ValueError('snapshot contents changed during the build')
        artifact = artifacts[0]
        executable = Path(artifact['executable'])
        frozen = output / executable.name
        with executable.open('rb') as source, frozen.open('xb') as destination:
            shutil.copyfileobj(source, destination)
        frozen.chmod(executable.stat().st_mode & 0o777)
        record.update(passed=True, cargo_artifact=artifact, binary=str(frozen),
                      binary_sha256=hashlib.sha256(frozen.read_bytes()).hexdigest())
    except Exception as error:
        record['error'] = str(error)
    finally:
        (output / 'build.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps({'passed': record['passed'], 'receipt': str(output / 'build.json'),
                      'error': record.get('error')}))
    return 0 if record['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
