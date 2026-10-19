#!/usr/bin/env python3
"""Check that the maintained repository ships only the Rust/CBOR product."""
from pathlib import Path
import json
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SUFFIXES = ('.c', '.cc', '.cpp', '.cxx', '.h', '.hh', '.hpp', '.hxx', '.lib', '.dll',
            '.so', '.dylib', '.a', '.sln', '.vcxproj', '.vcxproj.filters', '.vcxproj.user')
RETIRED = ('cpp/', 'uws/', 'include64/', 'lib64/', '.vs/', 'docs/benchmarks/')
OLD_FILES = {'app', 'relay.js', 'relay.d.ts', 'relay.module.js', 'relay.module.d.ts',
             'src/legacy.rs', 'src/legacy_bridge.rs', 'src/bin/war-legacy-bridge.rs'}


def main():
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others',
                                     '--exclude-standard'], cwd=ROOT).decode().split('\0')
    files = sorted({n for n in names if n and (ROOT / n).is_file()})
    invalid = [n for n in files if n.endswith(SUFFIXES) or n.startswith(RETIRED) or n in OLD_FILES]
    if invalid:
        raise SystemExit('Retired product files remain:\n' + '\n'.join(invalid))
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--no-deps', '--format-version=1'], cwd=ROOT))
    package = next(p for p in metadata['packages'] if p['name'] == 'war-relay')
    bins = [t['name'] for t in package['targets'] if 'bin' in t['kind']]
    if bins != ['war-relay']:
        raise SystemExit(f'Unexpected server targets: {bins}')
    print(f'Checked {len(files)} maintained files; one Rust server target, no retired native/protocol files.')


if __name__ == '__main__':
    main()
