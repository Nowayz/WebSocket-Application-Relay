#!/usr/bin/env python3
"""Verify local release archives, the installed client package and the packaged server."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

TOKEN = 'release-test-token'
GAUGES = ('war_connections', 'war_sessions', 'war_quic_connections', 'war_queued_bytes',
          'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes')


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def wait_ready(server, base):
    deadline = time.monotonic() + 5
    while True:
        if server.poll() is not None:
            raise ValueError('packaged server failed startup; see its server log')
        try:
            with urllib.request.urlopen(base + '/readyz', timeout=0.5) as response:
                if response.status != 200:
                    raise RuntimeError(f'packaged server readiness returned HTTP {response.status}')
            return
        except (OSError, urllib.error.URLError):
            if time.monotonic() >= deadline:
                raise TimeoutError('packaged server readiness deadline')
            time.sleep(0.05)


def request(base, path, authorized=True, method='GET'):
    headers = {'Authorization': f'Bearer {TOKEN}'} if authorized else {}
    try:
        with urllib.request.urlopen(urllib.request.Request(base + path, method=method, headers=headers),
                                    timeout=3) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def expect(base, path, status, authorized=True, method='GET'):
    observed, body = request(base, path, authorized, method)
    if observed != status:
        raise ValueError(f'{method} {path} returned HTTP {observed}, expected {status}')
    return body


def lifecycle(binary, env, log_path):
    """Start the packaged server with a development certificate and check its management lifecycle."""
    relay_port, admin_port = free_port(), free_port()
    while admin_port == relay_port:
        admin_port = free_port()
    base = f'http://127.0.0.1:{admin_port}'
    with log_path.open('x') as log:
        server = subprocess.Popen([binary, '--dev-cert', '--bind', f'127.0.0.1:{relay_port}',
                                   '--admin-bind', f'127.0.0.1:{admin_port}'],
                                  env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            wait_ready(server, base)
            # The WebTransport endpoint may log its pin just after management readiness.
            deadline = time.monotonic() + 5
            while not re.search(r'certificate_sha256=[0-9a-fA-F:]{95}',
                                re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', log_path.read_text())):
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise ValueError('packaged server did not report its WebTransport certificate pin')
                time.sleep(0.05)
            expect(base, '/metrics', 401, authorized=False)
            expect(base, '/v1/drain', 401, authorized=False, method='POST')
            if not isinstance(json.loads(expect(base, '/v1/channels?limit=1', 200)), list):
                raise ValueError('channel listing is not a JSON array')
            expect(base, '/v1/drain', 204, method='POST')
            expect(base, '/readyz', 503, authorized=False)
            expect(base, '/v1/resume', 204, method='POST')
            expect(base, '/readyz', 200, authorized=False)
            metrics = dict(line.split() for line in expect(base, '/metrics', 200).decode().splitlines()
                           if line.startswith('war_'))
            if any(metrics.get(name) != '0' for name in GAUGES):
                raise ValueError(f'resource gauges are not zero: {[(n, metrics.get(n)) for n in GAUGES]}')
            server.send_signal(signal.SIGTERM)
            status = server.wait(timeout=15)
            if status != 0:
                raise RuntimeError(f'packaged server exited with status {status} after SIGTERM')
        finally:
            if server.poll() is None:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True, help='New receipt directory')
    args = parser.parse_args()
    release = args.release.resolve()
    manifest = json.loads((release / 'release.json').read_text())
    if manifest['status'] != 'built':
        parser.error('release build did not succeed')
    try:
        args.output.mkdir(parents=True, exist_ok=False)
    except OSError as error:
        parser.error(str(error))
    output = args.output.resolve()
    record = {'passed': False, 'commit': manifest['commit'], 'checks': []}
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env.update(WAR_ADMIN_TOKEN=TOKEN, RUST_LOG='war_relay=info')
    try:
        sums = []
        for number, line in enumerate((release / 'SHA256SUMS').read_text().splitlines(), 1):
            digest, separator, name = line.partition('  ')
            if not (digest and separator and name):
                raise ValueError(f'SHA256SUMS line {number} is not a "digest  name" entry')
            sums.append((name, digest))
        if set(sums) != set(manifest['assets'].items()):
            raise ValueError('SHA256SUMS entries differ from the release.json assets')
        record['checks'].append('SHA256SUMS lists exactly the release.json assets with matching digests.')
        with tempfile.TemporaryDirectory(prefix='war-release-check-') as temporary:
            work = Path(temporary)
            for name, digest in manifest['assets'].items():
                if Path(name).name != name:
                    raise ValueError('asset names must be simple file names')
                with (release / name).open('rb') as source:
                    if hashlib.file_digest(source, 'sha256').hexdigest() != digest:
                        raise ValueError(f'{name}: SHA-256 digest differs from release.json')
                if name.endswith('.tar.gz'):
                    with tarfile.open(release / name) as archive:
                        archive.extractall(work, filter='data')
            record['checks'].append('All three release asset SHA-256 digests match.')
            version = manifest['version']
            runtime = work / f"war-relay-{version}-{manifest['host']}"
            source = work / f'war-relay-{version}-source'
            binary = runtime / 'bin/war-relay'
            identity = json.loads((runtime / 'release.json').read_text())
            if not all(identity[key] == manifest[key] for key in ('commit', 'version', 'host', 'tools')):
                raise ValueError('runtime archive release.json identity differs from the release manifest')
            build = json.loads((release / 'build/build.json').read_text())
            with binary.open('rb') as executable:
                if hashlib.file_digest(executable, 'sha256').hexdigest() != build['binary_sha256']:
                    raise ValueError('packaged server binary SHA-256 differs from build/build.json')
            reported = subprocess.check_output([binary, '--version'], text=True).strip()
            if reported != f'war-relay {version}':
                raise ValueError(f'packaged server reported {reported!r}, expected war-relay {version}')
            for name in ('browser/relay.js', 'browser/relay.d.ts', 'browser/index.html',
                         'deploy/war-relay.service', 'docs/migration.md', 'LICENSE'):
                if not (runtime / name).is_file():
                    raise ValueError(f'runtime archive is missing {name}')
            client_archive, = [release / name for name in manifest['assets'] if name.endswith('.tgz')]
            consumer = work / 'consumer'
            consumer.mkdir()
            (consumer / 'package.json').write_text('{"private":true,"type":"module"}\n')
            with (output / 'client.log').open('x') as log:
                subprocess.run(['npm', 'install', '--omit=optional', '--ignore-scripts', '--no-audit', '--no-fund',
                                str(client_archive)], cwd=consumer, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
            installed = consumer / 'node_modules/@war/relay-client'
            if (installed / 'LICENSE').read_bytes() != (runtime / 'LICENSE').read_bytes():
                raise ValueError('installed client LICENSE differs from the runtime LICENSE')
            if (installed / 'relay.d.ts').read_bytes() != (source / 'clients/js/relay.d.ts').read_bytes():
                raise ValueError('installed client relay.d.ts differs from the source archive')
            if (runtime / 'browser/LICENSE').read_bytes() != (runtime / 'LICENSE').read_bytes():
                raise ValueError('browser/LICENSE differs from the runtime LICENSE')
            if (runtime / 'browser/LICENSE.cbor-x').read_bytes() != (consumer / 'node_modules/cbor-x/LICENSE').read_bytes():
                raise ValueError('browser/LICENSE.cbor-x differs from the installed cbor-x LICENSE')
            notices = json.loads((runtime / 'third-party/index.json').read_text())
            if not notices:
                raise ValueError('third-party/index.json lists no dependency notices')
            for package in notices:
                for name, digest in package['files'].items():
                    with (runtime / 'third-party' / package['directory'] / name).open('rb') as notice:
                        if hashlib.file_digest(notice, 'sha256').hexdigest() != digest:
                            raise ValueError(f"third-party notice digest differs: {package['directory']}/{name}")
            record['checks'].append(f"License notices verified for the browser codec and {len(notices)} compiled Rust dependency packages.")
            tests = sorted((source / 'clients/js').glob('*.test.mjs'))
            if len(tests) != 3:
                raise ValueError(f'expected 3 client test files in the source archive, found {len(tests)}')
            for test in tests:
                shutil.copy2(test, installed / test.name)
            with (output / 'tests.log').open('x') as tests_log:
                subprocess.run(['node', '--test', '--test-timeout=10000', *[p.name for p in tests]],
                               cwd=installed, env=env, stdout=tests_log, stderr=subprocess.STDOUT, check=True, timeout=60)
            record['checks'].append('Packed client installed with license/types; all client tests passed against the installed package.')
            lifecycle(binary, env, output / 'server.log')
            record['checks'].append('Packaged server started WebTransport with a development certificate; management '
                                    'authentication, drain/resume, zero resources and graceful SIGTERM passed.')
            for profile in ('war-relay.env.example', 'war-relay-100k.env.example'):
                settings = dict(line.split('=', 1) for raw in (runtime / 'deploy' / profile).read_text().splitlines()
                                if (line := raw.strip()) and not line.startswith('#'))
                lifecycle(binary, env | settings, output / f'{profile}.server.log')
                record['checks'].append(f'{profile}: the packaged server accepted the profile and passed the same lifecycle.')
            record['passed'] = True
    except Exception as error:
        record['error'] = str(error)
    finally:
        (output / 'check.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return 0 if record['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
