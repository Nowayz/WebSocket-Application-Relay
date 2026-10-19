import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import test from 'node:test';
import { Relay } from './relay.mjs';

// Exercise the actual WebSocket close API, which mocks cannot validate.
async function peer(t, mode) {
  const sockets = new Set();
  let commands = 0;
  let closed;
  const closeCode = new Promise(resolve => { closed = resolve; });
  const server = createServer();
  t.after(() => { for (const socket of sockets) socket.destroy(); server.close(); });
  server.on('upgrade', (request, socket, head) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    const accept = createHash('sha1').update(request.headers['sec-websocket-key']
      + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
    socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
      + `Sec-WebSocket-Accept: ${accept}\r\nSec-WebSocket-Protocol: war.cbor.v1\r\n\r\n`);
    let buffered = head, messages = 0;
    const frame = (opcode, bytes) => socket.write(Buffer.from([0x80 | opcode, bytes.length, ...bytes]));
    socket.on('data', chunk => {
      buffered = Buffer.concat([buffered, chunk]);
      while (buffered.length >= 2) {
        const opcode = buffered[0] & 15;
        const length = buffered[1] & 127;
        assert.ok(buffered[1] & 128, 'client frames are masked');
        assert.ok(length < 126, 'fixture exchanges only short frames');
        if (buffered.length < 6 + length) return;
        const payload = Buffer.from(buffered.subarray(6, 6 + length));
        for (let i = 0; i < length; i++) payload[i] ^= buffered[2 + i % 4];
        buffered = buffered.subarray(6 + length);
        if (opcode === 8) {
          closed(payload.readUInt16BE());
          if (mode !== 'server-close') frame(8, payload);
          socket.end();
        } else if (opcode === 2) {
          if (++messages === 1) frame(2, mode === 'welcome' ? [0xf5] : [1]);
          else {
            commands++;
            if (mode === 'reply') frame(2, [0x82, 0, 0x82, 1, 0xf5]);
            else if (mode === 'server-close') frame(8, [3, 234]); // 1002
          }
        }
      }
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return { url: `ws://127.0.0.1:${server.address().port}`, closeCode, get commands() { return commands; } };
}

for (const mode of ['welcome', 'reply', 'timeout']) {
  test(`real WebSocket closes with application code 4002 for invalid ${mode}`, { timeout: 2000 }, async t => {
    const remote = await peer(t, mode);
    if (mode === 'welcome') {
      await assert.rejects(Relay.connect(remote.url, 'fault-peer'), /Invalid/);
    } else {
      const relay = await Relay.connect(remote.url, 'fault-peer');
      t.after(() => relay.close());
      if (mode === 'timeout') relay.requestTimeoutMs = 1;
      const pending = relay.listen(1);
      const queued = relay.listen(2);
      await Promise.all([
        assert.rejects(pending, mode === 'reply' ? /Invalid listener response/ : /request timeout/i),
        assert.rejects(queued, /Relay is closed/),
      ]);
    }
    assert.equal(await remote.closeCode, 4002);
    assert.equal(remote.commands, mode === 'welcome' ? 0 : 1, 'queued command must not reach the wire');
  });
}

test('real WebSocket preserves a server-sent protocol Close 1002', { timeout: 2000 }, async t => {
  const remote = await peer(t, 'server-close');
  const relay = await Relay.connect(remote.url, 'fault-peer');
  t.after(() => relay.close());
  await assert.rejects(relay.listen(1), /1002/);
  assert.equal(await remote.closeCode, 1002);
});
