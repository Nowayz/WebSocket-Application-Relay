import assert from 'node:assert/strict';
import { getEventListeners } from 'node:events';
import test from 'node:test';
import { Encoder, Decoder } from 'cbor-x';
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

class CaptureSocket extends Socket {
  send(bytes) { super.send(); this.subscription = Uint8Array.from(bytes); }
}

class CommandSocket extends Socket {
  packets = [];
  send(bytes) { super.send(); this.packets.push(new Decoder({ mapsAsObjects: true }).decode(Uint8Array.from(bytes))); }
  reply(op, result) {
    const bytes = new Encoder({ useRecords: false }).encode([0, [op, result]]);
    this.dispatchEvent(new MessageEvent('message', { data: Uint8Array.from(bytes).buffer }));
  }
}
const commandTurn = () => new Promise(resolve => setImmediate(resolve));

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

test('overflowing join timeout is rejected before creating a socket', async () => {
  class CountedSocket extends Socket {
    static created = 0;
    constructor() { super(); CountedSocket.created++; }
  }
  for (const requestTimeoutMs of [2147483648, Number.MAX_VALUE]) {
    await assert.rejects(Relay.connect('ws://fixture', 'timeout', {
      WebSocketClass: CountedSocket, requestTimeoutMs,
    }), RangeError);
    assert.equal(CountedSocket.created, 0);
  }
});

test('constructor rejects unsafe timeout configuration before installing listeners', () => {
  for (const timeout of [0, -1, NaN, Infinity, 2147483648]) {
    const socket = new Socket();
    assert.throws(() => new Relay(socket, 42n, timeout), RangeError);
    noSetupListeners(socket);
  }
});

test('changed timeout configuration rejects a command before sending and can recover', async () => {
  const socket = new Socket();
  const relay = new Relay(socket, 42n, 1000);
  try {
    relay.requestTimeoutMs = 2147483648;
    await assert.rejects(relay.channels(), RangeError);
    assert.equal(socket.sends, 0);
    assert.equal(socket.closeCalls, 0);
    assert.equal(relay.ready, true);
    relay.requestTimeoutMs = 1000;
    const command = relay.listen(1);
    await Promise.resolve();
    socket.dispatchEvent(new MessageEvent('message', {
      data: Uint8Array.from([0x82, 0, 0x82, 1, 1]).buffer,
    }));
    assert.equal(await command, 1);
  } finally { relay.close(); }
});

test('timer ceiling is inclusive and direct construction uses the standard default', async () => {
  const direct = new Relay(new Socket(), 42n);
  assert.equal(direct.requestTimeoutMs, 10000);
  direct.close();
  for (const requestTimeoutMs of [0.5, 1, 2147483647]) {
    const pending = Relay.connect('ws://fixture', 'timeout', {
      WebSocketClass: Socket, requestTimeoutMs,
    });
    const socket = Socket.instance;
    socket.dispatchEvent(new Event('open'));
    socket.welcome();
    const relay = await pending;
    assert.equal(relay.requestTimeoutMs, requestTimeoutMs);
    relay.close();
  }
});

test('connect captures a byte channel before the delayed socket open', async () => {
  const backing = new Uint8Array([99, 97, 98, 99, 99]);
  const channel = backing.subarray(1, 4);
  const pending = Relay.connect('ws://fixture', channel, { WebSocketClass: CaptureSocket });
  const socket = Socket.instance;
  channel.fill(120);
  socket.dispatchEvent(new Event('open'));
  socket.welcome();
  const relay = await pending;
  try {
    assert.deepEqual(socket.subscription, new Uint8Array([0x43, 97, 98, 99]));
  } finally { relay.close(); }
});

test('connect does not depend on the channel buffer remaining attached', async () => {
  const channel = new Uint8Array([97, 98, 99]);
  const pending = Relay.connect('ws://fixture', channel, { WebSocketClass: CaptureSocket });
  const socket = Socket.instance;
  structuredClone(channel.buffer, { transfer: [channel.buffer] });
  socket.dispatchEvent(new Event('open'));
  socket.welcome();
  const relay = await pending;
  try {
    assert.deepEqual(socket.subscription, new Uint8Array([0x43, 97, 98, 99]));
  } finally { relay.close(); }
});

test('prepared subscriptions survive concurrent joins and encoder buffer growth', async () => {
  const first = Relay.connect('ws://fixture', new Uint8Array([1, 2]), { WebSocketClass: CaptureSocket });
  const a = Socket.instance;
  const second = Relay.connect('ws://fixture', new Uint8Array([3, 4]), { WebSocketClass: CaptureSocket });
  const b = Socket.instance;
  const unrelated = new Relay(new Socket(), 50n);
  unrelated.send(51n, new Uint8Array(65536));
  unrelated.close();
  for (const socket of [b, a]) {
    socket.dispatchEvent(new Event('open'));
    socket.welcome();
  }
  const relays = await Promise.all([first, second]);
  try {
    assert.deepEqual(a.subscription, new Uint8Array([0x42, 1, 2]));
    assert.deepEqual(b.subscription, new Uint8Array([0x42, 3, 4]));
  } finally { for (const relay of relays) relay.close(); }
});

test('commands capture nested values before the next microtask sends them', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  const value = { bytes: new Uint8Array([1, 2]), tags: ['before'] };
  const write = relay.setVariable('snapshot', value);
  write.catch(() => {});
  value.bytes.fill(9);
  value.tags.push('after');
  try {
    await commandTurn();
    assert.deepEqual(socket.packets, [[0n, [4, ['snapshot', { bytes: new Uint8Array([1, 2]), tags: ['before'] }]]]]);
    socket.reply(4, true);
    assert.equal(await write, true);
  } finally { relay.close(); await Promise.allSettled([write]); }
});

test('a queued command survives transfer of its original value buffer', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  const first = relay.listen(0);
  const bytes = new Uint8Array([3, 4]);
  const queued = relay.setVariable('transfer', bytes);
  queued.catch(() => {});
  structuredClone(bytes.buffer, { transfer: [bytes.buffer] });
  try {
    await commandTurn();
    assert.deepEqual(socket.packets, [[0n, [1, 0]]]);
    socket.reply(1, 0);
    await first;
    await commandTurn();
    assert.equal(socket.packets.length, 2);
    assert.deepEqual(socket.packets[1], [0n, [4, ['transfer', new Uint8Array([3, 4])]]]);
    socket.reply(4, true);
    assert.equal(await queued, true);
  } finally { relay.close(); await Promise.allSettled([first, queued]); }
});

test('reentrant value getters preserve outer command order and encode once', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  let nested, reads = 0;
  const outer = relay.setVariable('getter', { get child() {
    reads++;
    nested = relay.listen(2);
    return 'value';
  } });
  try {
    await commandTurn();
    assert.equal(reads, 1);
    assert.deepEqual(socket.packets, [[0n, [4, ['getter', { child: 'value' }]]]]);
    socket.reply(4, true);
    await outer;
    await commandTurn();
    assert.deepEqual(socket.packets[1], [0n, [1, 2]]);
    socket.reply(1, 2);
    assert.equal(await nested, 2);
  } finally { relay.close(); await Promise.allSettled([outer, nested]); }
});

test('encoding failure retains its queue position and allows the next command', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  const failure = new Error('controlled encoding failure');
  const first = relay.listen(0);
  let bad;
  assert.doesNotThrow(() => { bad = relay.setVariable('bad', { get child() { throw failure; } }); });
  let rejected = false;
  const checked = assert.rejects(bad, error => { rejected = true; return error === failure; });
  const last = relay.listen(2);
  try {
    await commandTurn();
    assert.equal(rejected, false);
    assert.equal(socket.packets.length, 1);
    socket.reply(1, 0);
    await first;
    await checked;
    await commandTurn();
    assert.deepEqual(socket.packets, [[0n, [1, 0]], [0n, [1, 2]]]);
    socket.reply(1, 2);
    assert.equal(await last, 2);
    assert.equal(socket.closeCalls, 0);
  } finally { relay.close(); await Promise.allSettled([first, bad, last]); }
});

test('a reentrant queued write captures its buffer without corrupting the outer encoder', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  const bytes = new Uint8Array([5, 6]);
  let nested;
  const outer = relay.setVariable('outer', { get child() {
    nested = relay.setVariable('inner', bytes);
    bytes.fill(9);
    return 'value';
  } });
  try {
    await commandTurn();
    assert.deepEqual(socket.packets, [[0n, [4, ['outer', { child: 'value' }]]]]);
    socket.reply(4, true);
    await outer;
    await commandTurn();
    assert.deepEqual(socket.packets[1], [0n, [4, ['inner', new Uint8Array([5, 6])]]]);
    socket.reply(4, true);
    assert.equal(await nested, true);
  } finally { relay.close(); await Promise.allSettled([outer, nested]); }
});

test('a closed client rejects commands without evaluating their values', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  relay.close();
  let reads = 0;
  await assert.rejects(relay.setVariable('closed', { get child() { reads++; return 'value'; } }), /Relay is closed/);
  assert.equal(reads, 0);
  assert.equal(socket.packets.length, 0);
});

test('a command invoked from an application-value getter waits for that encoding', async () => {
  const socket = new CommandSocket();
  const relay = new Relay(socket, 42n);
  const bytes = new Uint8Array([7, 8]);
  let nested;
  relay.send(99n, { get child() {
    nested = relay.setVariable('during-send', bytes);
    bytes.fill(9);
    return 'application';
  } });
  try {
    await commandTurn();
    assert.deepEqual(socket.packets, [
      [99n, { child: 'application' }],
      [0n, [4, ['during-send', new Uint8Array([7, 8])]]],
    ]);
    socket.reply(4, true);
    assert.equal(await nested, true);
  } finally { relay.close(); await Promise.allSettled([nested]); }
});
