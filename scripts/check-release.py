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
        with tempfile.TemporaryDirectory(prefix='war-release-check-') as temporary:
            work = Path(temporary)
            for name, digest in manifest['assets'].items():
                if Path(name).name != name:
                    raise ValueError('asset names must be simple file names')
                with (release / name).open('rb') as source:
                    assert hashlib.file_digest(source, 'sha256').hexdigest() == digest, name
                if name.endswith('.tar.gz'):
                    with tarfile.open(release / name) as archive:
                        archive.extractall(work, filter='data')
            record['checks'].append('All three release asset SHA-256 digests match.')
            version = manifest['version']
            runtime = work / f"war-relay-{version}-{manifest['host']}"
            source = work / f'war-relay-{version}-source'
            binary = runtime / 'bin/war-relay'
            identity = json.loads((runtime / 'release.json').read_text())
            assert all(identity[key] == manifest[key] for key in ('commit', 'version', 'host', 'tools'))
            build = json.loads((release / 'build/build.json').read_text())
            with binary.open('rb') as executable:
                assert hashlib.file_digest(executable, 'sha256').hexdigest() == build['binary_sha256']
            assert subprocess.check_output([binary, '--version'], text=True).strip() == f'war-relay {version}'
            for name in ('browser/relay.js', 'browser/relay.d.ts', 'browser/index.html',
                         'deploy/war-relay.service', 'docs/migration.md', 'LICENSE'):
                assert (runtime / name).is_file(), name
            client_archive, = [release / name for name in manifest['assets'] if name.endswith('.tgz')]
            consumer = work / 'consumer'
            consumer.mkdir()
            (consumer / 'package.json').write_text('{"private":true,"type":"module"}\n')
            with (output / 'client.log').open('x') as log:
                subprocess.run(['npm', 'install', '--omit=optional', '--ignore-scripts', '--no-audit', '--no-fund',
                                str(client_archive)], cwd=consumer, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=120)
            installed = consumer / 'node_modules/@war/relay-client'
            assert (installed / 'LICENSE').read_bytes() == (runtime / 'LICENSE').read_bytes()
            assert (installed / 'relay.d.ts').read_bytes() == (source / 'clients/js/relay.d.ts').read_bytes()
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
                deadline = time.monotonic() + 5
                while True:
                    if server.poll() is not None:
                        raise ValueError('packaged server failed startup; see server.log')
                    try:
                        with urllib.request.urlopen(env['WAR_TEST_ADMIN'] + '/readyz', timeout=0.5) as response:
                            assert response.status == 200
                        break
                    except (OSError, urllib.error.URLError):
                        if time.monotonic() >= deadline:
                            raise TimeoutError('packaged server readiness deadline')
                        time.sleep(0.05)
                tests = sorted((source / 'clients/js').glob('*.test.mjs'))
                assert len(tests) == 4
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
                assert server.wait(timeout=15) == 0
                record['checks'].append('Package exports, management authentication, drain/resume, zero resources and graceful SIGTERM passed.')
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
