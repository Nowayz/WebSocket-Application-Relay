import assert from 'node:assert/strict';
import { getEventListeners } from 'node:events';
import test from 'node:test';
import { Encoder } from 'cbor-x';
import { Relay } from './relay.mjs';

class Socket extends EventTarget {
  static instance;
  readyState = 1;
  closeCalls = 0;
  sends = 0;
  constructor() { super(); Socket.instance = this; }
  send() { this.sends++; }
  // Keep Close delivery separate from the local close request.
  close() { this.closeCalls++; this.readyState = 2; }
  welcome() {
    const bytes = new Encoder({ useRecords: false }).encode(42);
    this.dispatchEvent(new MessageEvent('message', { data: Uint8Array.from(bytes).buffer }));
  }
}

function noSetupListeners(socket) {
  for (const type of ['open', 'message', 'close', 'error']) {
    assert.equal(getEventListeners(socket, type).length, 0, type);
  }
}

test('failed connection removes setup listeners before delayed socket events', async () => {
  for (const type of ['error', 'close']) {
    const pending = Relay.connect('ws://fixture', 'lifecycle', { WebSocketClass: Socket });
    const socket = Socket.instance;
    socket.dispatchEvent(new Event(type));
    await assert.rejects(pending, /Relay closed before subscription/);
    noSetupListeners(socket);
    assert.equal(socket.closeCalls, 1);
    socket.dispatchEvent(new Event('open'));
    socket.welcome();
    assert.equal(socket.sends, 0);
    noSetupListeners(socket);
  }
});

test('join timeout removes listeners and closes without a peer Close event', async () => {
  const pending = Relay.connect('ws://fixture', 'lifecycle', {
    WebSocketClass: Socket, requestTimeoutMs: 1,
  });
  const socket = Socket.instance;
  await assert.rejects(pending, /Relay join timeout/);
  assert.equal(socket.closeCalls, 1);
  noSetupListeners(socket);
});

test('join send failure rejects immediately with its original error', async () => {
  const failure = new Error('controlled join send failure');
  class FailingSocket extends Socket { send() { throw failure; } }
  const pending = Relay.connect('ws://fixture', 'lifecycle', { WebSocketClass: FailingSocket });
  const socket = Socket.instance;
  socket.dispatchEvent(new Event('open'));
  await assert.rejects(pending, error => error === failure);
  assert.equal(socket.closeCalls, 1);
  noSetupListeners(socket);
});

test('successful welcome replaces setup listeners with the live relay listeners', async () => {
  const pending = Relay.connect('ws://fixture', 'lifecycle', { WebSocketClass: Socket });
  const socket = Socket.instance;
  socket.dispatchEvent(new Event('open'));
  socket.welcome();
  const relay = await pending;
  assert.equal(relay.userId, 42n);
  assert.equal(socket.sends, 1);
  for (const [type, count] of [['open', 0], ['error', 0], ['message', 1], ['close', 1]]) {
    assert.equal(getEventListeners(socket, type).length, count, type);
  }
  relay.close();
});
