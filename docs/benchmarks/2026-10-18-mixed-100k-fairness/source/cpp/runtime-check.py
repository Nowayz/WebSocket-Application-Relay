#!/usr/bin/env python3
"""Check observer/control semantics, active shutdown and failed startup."""
import importlib.util, json, os, pathlib, socket, subprocess, time, traceback
ROOT = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('corpus', ROOT / 'scripts/input-corpus.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
exe = ROOT / 'artifacts/candidates/cpp-modern/war-relay-cpp'
rows = []
env = {k: v for k, v in os.environ.items() if not k.startswith('WAR_')}
env['WAR_AUTH_PASSWORD'] = 'cpp-test-auth'
env['WAR_ADMIN_TOKEN'] = c.TOKEN
port, admin = (c.wire.runtime.free_port(), c.wire.runtime.free_port())
cmd = [str(exe), '--bind', f'127.0.0.1:{port}', '--admin-bind', f'127.0.0.1:{admin}', '--workers', '4']

def packet(target, value):
    return b'\x82\x1b' + target.to_bytes(8, 'big') + value

def send(peer, target, value):
    peer.socket.sendall(c.wire.frame(130, packet(target, value)))

def control(peer, op, value):
    send(peer, 0, bytes([130, op]) + value)

def auth(peer):
    control(peer, 0, b'Mcpp-test-auth')
    assert peer.receive() == (2, b'\x82\x00\x82\x00\xf5')

def expected(sender, value):
    return (2, packet(sender, value))
peers = []
with open(ROOT / 'artifacts/cpp-modern-runtime-server.log', 'w') as log:
    server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
    try:
        for _ in range(200):
            try:
                c.metrics(admin)
                break
            except OSError:
                time.sleep(0.01)

        def peer(channel):
            p = c.Peer(port)
            p.join(channel)
            peers.append(p)
            return p
        a, b, g = (peer(b'room'), peer(b'room'), peer(b're_globl'))
        auth(g)
        control(g, 1, b'\x07')
        assert g.receive() == (2, b'\x82\x00\x82\x01\x07')
        value = b'Cabc'
        send(a, b.id, value)
        assert b.receive() == g.receive() == expected(a.id, value)
        send(a, g.id, value)
        assert g.receive() == expected(a.id, value)
        send(g, g.id, b'\x01')
        assert g.receive() == expected(g.id, b'\x01')
        send(a, 2 ** 64 - 1, value)
        assert b.receive() == g.receive() == expected(a.id, value)
        control(g, 1, b'\x00')
        assert g.receive() == (2, b'\x82\x00\x82\x01\x00')
        send(a, b.id, value)
        assert b.receive() == expected(a.id, value)
        send(g, g.id, b'\x02')
        assert g.receive() == expected(g.id, b'\x02')
        control(g, 1, b'\x07')
        assert g.receive() == (2, b'\x82\x00\x82\x01\x07')
        b.close()
        peers.remove(b)
        notice = expected(2 ** 64 - 1, b'\x1b' + b.id.to_bytes(8, 'big'))
        assert a.receive() == g.receive() == notice
        control(a, 4, b'\x82AkBhi')
        assert a.receive() == (2, b'\x82\x00\x82\x04\xf5')
        control(a, 5, b'Ak')
        assert a.receive() == (2, b'\x82\x00\x82\x18\xc8\x82\xf5Bhi')
        other = peer(b'other')
        control(other, 5, b'Ak')
        assert other.receive() == (2, b'\x82\x00\x82\x18\xc8\x82\xf4\xf6')
        rows.append({'case': 'observer-scope-dedup-optout-variables-disconnect', 'passed': True})
        auth(a)
        auth(other)
        assigned = 8070450532247928849
        control(a, 3, b'\x1b' + assigned.to_bytes(8, 'big'))
        assert a.receive() == (2, b'\x82\x00\x82\x03\x1b' + assigned.to_bytes(8, 'big'))
        a.id = assigned
        control(other, 3, b'\x1b' + assigned.to_bytes(8, 'big'))
        assert other.receive() == (2, b'\x82\x00\x82\x03\x1b' + assigned.to_bytes(8, 'big'))
        other.id = assigned
        opcode, body = a.receive()
        assert opcode == 8 and int.from_bytes(body[:2], 'big') == 4001 and (body[2:] == b'UserID Taken')
        a.socket.sendall(c.wire.frame(136, body))
        assert a.socket.recv(1) == b''
        a.socket.close()
        peers.remove(a)
        send(other, other.id, value)
        assert other.receive() == expected(assigned, value)
        rows.append({'case': 'identity-takeover-owner-retirement', 'passed': True})
        denied = peer(b'denied')
        control(denied, 1, b'\x01')
        op, body = denied.receive()
        assert op == 8 and body == b'\x03\xf0Authentication Required'
        denied.socket.close()
        peers.remove(denied)
        rows.append({'case': 'unauthorized-listen-close', 'passed': True})
    finally:
        for p in peers:
            p.socket.close()
        server.terminate()
        assert server.wait(timeout=10) == 0
for phase in ['active', 'half-http']:
    port = c.wire.runtime.free_port()
    cmd = [str(exe), '--bind', f'127.0.0.1:{port}', '--workers', '4', '--no-admin']
    load = None
    half = None
    with open(ROOT / f'artifacts/cpp-modern-shutdown-{phase}.log', 'w') as log:
        server = subprocess.Popen(cmd, env=env, stdout=log, stderr=log)
        try:
            time.sleep(0.1)
            if phase == 'active':
                load = subprocess.Popen([str(ROOT / 'artifacts/baselines/capacity-final/stress'), '--url', f'ws://127.0.0.1:{port}', '--mode', 'traffic', '--clients', '64', '--duration-secs', '30'], stdout=log, stderr=log)
            else:
                half = socket.create_connection(('127.0.0.1', port))
                half.sendall(b'GET / HTTP/1.1\r\nHost:')
            time.sleep(0.15)
            server.terminate()
            assert server.wait(timeout=10) == 0
            rows.append({'case': 'shutdown-' + phase, 'passed': True})
        finally:
            if server.poll() is None:
                server.kill()
                server.wait()
            if load and load.poll() is None:
                load.kill()
                load.wait()
            if half:
                half.close()
with socket.socket() as occupied:
    occupied.bind(('127.0.0.1', 0))
    occupied.listen()
    port = occupied.getsockname()[1]
    result = subprocess.run([str(exe), '--bind', f'127.0.0.1:{port}', '--workers', '4', '--no-admin'], env=env, capture_output=True, text=True, timeout=10)
    assert result.returncode == 1 and 'listener bind' in result.stderr, result
    rows.append({'case': 'failed-bind-unwinds-workers', 'passed': True})
record = {'passed': True, 'checks': rows}
(ROOT / 'artifacts/cpp-modern-runtime-check.json').write_text(json.dumps(record, indent=2) + '\n')
print(json.dumps(record))
