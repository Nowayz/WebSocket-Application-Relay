#!/usr/bin/env python3
"""Verify local release archives using an installed client and the packaged server."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.request


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
    server = None
    env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
    env.update(WAR_AUTH_PASSWORD='js-test-host', WAR_ADMIN_TOKEN='release-test-token')
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
            relay_port, admin_port = free_port(), free_port()
            while admin_port == relay_port:
                admin_port = free_port()
            env['WAR_TEST_URL'] = f'ws://127.0.0.1:{relay_port}'
            env['WAR_TEST_IDLE_ENABLED'] = '1'
            env['WAR_TEST_ADMIN'] = f'http://127.0.0.1:{admin_port}'
            with (output / 'server.log').open('x') as log:
                server = subprocess.Popen([binary, '--bind', f'127.0.0.1:{relay_port}', '--admin-bind',
                                           f'127.0.0.1:{admin_port}', '--workers', '2', '--idle-timeout-secs', '1'],
                                          env=env, stdout=log, stderr=subprocess.STDOUT)
                wait_ready(server, env['WAR_TEST_ADMIN'])
                tests = sorted((source / 'clients/js').glob('*.test.mjs'))
                if len(tests) != 4:
                    raise ValueError(f'expected 4 client test files in the source archive, found {len(tests)}')
                for test in tests:
                    shutil.copy2(test, installed / test.name)
                with (output / 'tests.log').open('x') as tests_log:
                    subprocess.run(['node', '--test', '--test-timeout=10000', *[p.name for p in tests]],
                                   cwd=installed, env=env, stdout=tests_log, stderr=subprocess.STDOUT, check=True, timeout=60)
                record['checks'].append('Packed client installed with license/types; all client tests passed against the packaged server.')
                # This imports through package exports from an unrelated consumer directory.
                # Keep a session across drain/resume, then verify terminal Close on SIGTERM.
                smoke = r'''
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { Relay } from '@war/relay-client';
const base = process.env.WAR_TEST_ADMIN;
const request = (path, authorized = true, method = 'GET') => fetch(base + path, {
  method, signal: AbortSignal.timeout(3000),
  headers: authorized ? { authorization: 'Bearer release-test-token' } : {},
});
const ready = async status => assert.equal((await request('/readyz', false)).status, status);
if (process.env.WAR_TEST_LARGE_PROFILE) {
  let reused = 0, copied = 0;
  for (let attempt = 0; attempt < 100; attempt++) {
    const response = await request('/metrics'); assert.equal(response.status, 200);
    const metrics = new Map((await response.text()).split('\n').filter(l => l && !l.startsWith('#')).map(l => l.split(' ')));
    reused = Number(metrics.get('war_routing_reused_bytes_total'));
    copied = Number(metrics.get('war_routing_copied_bytes_total'));
    if (reused >= 10 * 1024 * 1024) break;
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  assert(reused >= 10 * 1024 * 1024, 'both 5 MiB publications must reuse their received values');
  assert(copied < 1024, 'large file bodies were copied while routing');
  console.log(JSON.stringify({ largeFileReusedBytes: reused, compactedSmallBytes: copied }));
}
assert.equal((await request('/metrics', false)).status, 401);
assert.equal((await request('/v1/drain', false, 'POST')).status, 401);
const a = await Relay.connect(process.env.WAR_TEST_URL, 'release-smoke');
assert.equal(typeof a.userId, 'bigint');
const listing = await request('/v1/channels?limit=1');
assert.equal(listing.status, 200);
assert.equal((await listing.json()).length, 1);
assert.equal((await request('/v1/drain', true, 'POST')).status, 204);
await ready(503);
const echoed = once(a, 'message');
a.send(a.userId, { drained: true, bytes: new Uint8Array([0, 255]) });
assert.deepEqual((await echoed)[0].detail.value, { drained: true, bytes: new Uint8Array([0, 255]) });
await assert.rejects(Relay.connect(process.env.WAR_TEST_URL, 'drained', { requestTimeoutMs: 1000 }));
assert.equal((await request('/v1/resume', true, 'POST')).status, 204);
await ready(200);
const b = await Relay.connect(process.env.WAR_TEST_URL, 'release-resume');
for (const client of [a, b]) {
  const closed = once(client.ws, 'close'); client.close();
  const [event] = await closed; assert(event.wasClean); assert.equal(event.code, 1000);
}
const gauges = ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes',
                'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes'];
let clean = false;
for (let retry = 0; retry < 100; retry++) {
  const response = await request('/metrics'); assert.equal(response.status, 200);
  const metrics = new Map((await response.text()).split('\n').filter(l => l && !l.startsWith('#')).map(l => l.split(' ')));
  if (gauges.every(name => metrics.get(name) === '0')) { clean = true; break; }
  await new Promise(resolve => setTimeout(resolve, 20));
}
assert(clean, 'release fixture resources did not retire');
const terminal = await Relay.connect(process.env.WAR_TEST_URL, 'release-stop');
const stopped = once(terminal.ws, 'close');
process.kill(Number(process.argv[1]), 'SIGTERM');
const [event] = await stopped;
assert.equal(event.code, 1001); assert(event.wasClean);
console.log('Package exports, admin auth, pagination, drain/rejected admission/resume, six zero gauges and SIGTERM Close 1001 passed.');
'''
                with (output / 'lifecycle.log').open('x') as lifecycle:
                    subprocess.run(['node', '--input-type=module', '-e', smoke, str(server.pid)], cwd=consumer,
                                   env=env, stdout=lifecycle, stderr=subprocess.STDOUT, check=True, timeout=30)
                status = server.wait(timeout=15)
                if status != 0:
                    raise RuntimeError(f'packaged server exited with status {status} after SIGTERM')
                record['checks'].append('Package exports, management authentication, drain/resume, zero resources and graceful SIGTERM passed.')
            for profile in ('war-relay.env.example', 'war-relay-100k.env.example'):
                settings = dict(line.split('=', 1) for raw in (runtime / 'deploy' / profile).read_text().splitlines()
                                if (line := raw.strip()) and not line.startswith('#'))
                profile_env = env | settings
                profile_env.pop('WAR_TEST_IDLE_ENABLED', None)
                relay_port, admin_port = free_port(), free_port()
                while admin_port == relay_port:
                    admin_port = free_port()
                profile_env.update(WAR_TEST_URL=f'ws://127.0.0.1:{relay_port}',
                                   WAR_TEST_ADMIN=f'http://127.0.0.1:{admin_port}', WAR_TEST_LARGE_PROFILE='1')
                with (output / f'{profile}.server.log').open('x') as log:
                    server = subprocess.Popen([binary, '--bind', f'127.0.0.1:{relay_port}',
                                               '--admin-bind', f'127.0.0.1:{admin_port}'],
                                              env=profile_env, stdout=log, stderr=subprocess.STDOUT)
                    wait_ready(server, profile_env['WAR_TEST_ADMIN'])
                    tests_log = output / f'{profile}.tests.log'
                    with tests_log.open('x') as tests_output:
                        subprocess.run(['node', '--test', '--test-timeout=10000', '--test-reporter=tap',
                                        '--test-name-pattern=native 5 MiB file values', 'relay.test.mjs'],
                                       cwd=installed, env=profile_env, stdout=tests_output,
                                       stderr=subprocess.STDOUT, check=True, timeout=30)
                    if '# pass 1' not in tests_log.read_text().splitlines():
                        raise ValueError(f'{profile}: large-file profile test did not run')
                    with (output / f'{profile}.lifecycle.log').open('x') as lifecycle:
                        subprocess.run(['node', '--input-type=module', '-e', smoke, str(server.pid)],
                                       cwd=consumer, env=profile_env, stdout=lifecycle,
                                       stderr=subprocess.STDOUT, check=True, timeout=30)
                    status = server.wait(timeout=15)
                    if status != 0:
                        raise RuntimeError(f'{profile}: packaged server exited with status {status} after SIGTERM')
                record['checks'].append(f'{profile}: 5 MiB private/room FIFO, shared large values, zero resources and graceful shutdown passed.')
            record['passed'] = True
    except Exception as error:
        record['error'] = str(error)
    finally:
        if server is not None and server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=15)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
        (output / 'check.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record, indent=2))
    return 0 if record['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
