import assert from 'node:assert/strict';
import test from 'node:test';
import { Encoder } from 'cbor-x';
import { Relay, BROADCAST } from './relay.mjs';

const encoder = new Encoder({ useRecords: false, tagUint8Array: false });
function socketClass(welcome) {
  return class extends EventTarget {
    static instance;
    readyState = 1;
    constructor() {
      super();
      this.constructor.instance = this;
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }
    send() {
      if (!this.joined) {
        this.joined = true;
        queueMicrotask(() => this.receive(welcome));
      }
    }
    receive(value) {
      const bytes = encoder.encode(value);
      const data = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
      this.dispatchEvent(new MessageEvent('message', { data }));
    }
    close(code = 1000, reason = '') {
      this.closed = { code, reason };
      this.readyState = 3;
      this.dispatchEvent(Object.assign(new Event('close'), { code, reason }));
    }
  };
}

const invalidSessions = [0, BROADCAST, -1, -1n, BROADCAST + 1n, '1', true, null,
  1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1];

test('welcome IDs must be exact, nonreserved uint64 integers', async () => {
  for (const id of invalidSessions) {
    const WebSocketClass = socketClass(id);
    await assert.rejects(Relay.connect('ws://fixture', 'id-check', { WebSocketClass, requestTimeoutMs: 1000 }), undefined, String(id));
    assert.equal(WebSocketClass.instance.closed.code, 4002);
  }
  for (const id of [1, Number.MAX_SAFE_INTEGER, 1n << 53n, BROADCAST - 1n]) {
    const WebSocketClass = socketClass(id);
    const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
    assert.equal(relay.userId, BigInt(id));
    relay.close();
  }
});

test('malformed sender and disconnect IDs close without dispatching application events', async () => {
  for (const notification of [false, true]) {
    const invalid = notification ? invalidSessions : invalidSessions.filter(id => id !== 0 && id !== BROADCAST);
    for (const id of invalid) {
      const WebSocketClass = socketClass(1);
      const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
      let delivered = 0, errors = 0;
      relay.addEventListener('message', () => delivered++);
      relay.addEventListener('disconnect', () => delivered++);
      relay.addEventListener('error', () => errors++);
      WebSocketClass.instance.receive(notification ? [BROADCAST, id] : [id, 'payload']);
      assert.equal(delivered, 0, `${notification}: ${String(id)}`);
      assert.equal(errors, 1);
      assert.equal(WebSocketClass.instance.closed.code, 4002);
    }
  }
});

test('valid large sender and disconnect IDs retain exact BigInt values', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
  for (const id of [1, Number.MAX_SAFE_INTEGER, 1n << 53n, BROADCAST - 1n]) {
    let message, disconnect;
    relay.addEventListener('message', event => { message = event.detail; }, { once: true });
    relay.addEventListener('disconnect', event => { disconnect = event.detail; }, { once: true });
    WebSocketClass.instance.receive([id, 'payload']);
    WebSocketClass.instance.receive([BROADCAST, id]);
    assert.deepEqual(message, { sender: BigInt(id), value: 'payload' });
    assert.deepEqual(disconnect, { userId: BigInt(id) });
  }
  relay.close();
});

test('invalid assignment acknowledgements reject the command and close the connection', async () => {
  for (const id of invalidSessions) {
    const WebSocketClass = socketClass(1);
    const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
    let errors = 0;
    relay.addEventListener('error', () => errors++);
    const assignment = relay.assignId(42n);
    await Promise.resolve();
    WebSocketClass.instance.receive([0, [3, id]]);
    await assert.rejects(assignment);
    assert.equal(relay.userId, 1n);
    assert.equal(errors, 1);
    assert.equal(WebSocketClass.instance.closed.code, 4002);
  }
});

test('valid assignment acknowledgement keeps the BigInt API', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
  const id = BROADCAST - 1n;
  const assignment = relay.assignId(id);
  await Promise.resolve();
  WebSocketClass.instance.receive([0, [3, id]]);
  assert.equal(await assignment, id);
  assert.equal(relay.userId, id);
  relay.close();
});

test('local close rejects pending and queued commands before a peer Close event', async () => {
  class DelayedClose extends socketClass(1) {
    close(code = 1000, reason = '') {
      this.closed = { code, reason };
      this.readyState = 2;
    }
  }
  const relay = await Relay.connect('ws://fixture', 'close-check', {
    WebSocketClass: DelayedClose, requestTimeoutMs: 1000,
  });
  let firstError, queuedError;
  const first = relay.channels().catch(error => { firstError = error; });
  await Promise.resolve();
  const queued = relay.listen(1).catch(error => { queuedError = error; });
  const ws = DelayedClose.instance;
  try {
    relay.close();
    await new Promise(setImmediate);
    assert.match(firstError?.message ?? '', /Relay is closed/);
    assert.match(queuedError?.message ?? '', /Relay is closed/);
    assert.equal(ws.closed.code, 1000);
    assert.equal(ws.readyState, 2);
  } finally {
    ws.dispatchEvent(Object.assign(new Event('close'), { code: 1000, reason: 'fixture cleanup' }));
    await Promise.all([first, queued]);
  }
});

test('malformed replies reject pending commands before the close handshake completes', async () => {
  const WebSocketClass = class extends socketClass(1) {
    close(code, reason) {
      this.closed = { code, reason };
      this.readyState = 2; // Peer withholds the Close acknowledgement.
    }
  };
  const relay = await Relay.connect('ws://fixture', 'id-check', { WebSocketClass });
  const pending = relay.listen(1);
  let rejection;
  const settled = pending.catch(error => { rejection = error; });
  await Promise.resolve();
  const ws = WebSocketClass.instance;
  try {
    ws.receive([0, [99, null]]); // Does not match the outstanding operation.
    await new Promise(setImmediate);
    assert.ok(rejection instanceof Error, 'command waited for Close or its request timeout');
    assert.match(rejection.message, /Unexpected relay response/);
    assert.equal(ws.closed.code, 4002);
    assert.equal(ws.readyState, 2);
  } finally {
    ws.dispatchEvent(Object.assign(new Event('close'), { code: 4002, reason: 'fixture cleanup' }));
    await settled;
  }
});

test('envelopes require exactly two array elements, not iterable byte strings', async () => {
  for (const malformed of [new Uint8Array([1, 2]), [1], [1, 'payload', 'extra'], [], null, 1, {}]) {
    const WebSocketClass = socketClass(1);
    const relay = await Relay.connect('ws://fixture', 'shape-check', { WebSocketClass });
    let delivered = 0, errors = 0;
    relay.addEventListener('message', () => delivered++);
    relay.addEventListener('error', () => errors++);
    try {
      WebSocketClass.instance.receive(malformed);
      assert.equal(delivered, 0);
      assert.equal(errors, 1);
      assert.equal(WebSocketClass.instance.closed.code, 4002);
    } finally { relay.close(); }
  }
});

test('control replies require exactly two array elements before resolving a command', async () => {
  for (const malformed of [new Uint8Array([1, 1]), [1], [1, 1, 'extra'], null, 1, {}]) {
    const WebSocketClass = socketClass(1);
    const relay = await Relay.connect('ws://fixture', 'shape-check', { WebSocketClass });
    const pending = relay.listen(1);
    await Promise.resolve();
    try {
      WebSocketClass.instance.receive([0, malformed]);
      await assert.rejects(pending);
      assert.equal(WebSocketClass.instance.closed.code, 4002);
    } finally { relay.close(); }
  }
});

test('valid envelopes preserve arbitrary application values including undefined', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'shape-check', { WebSocketClass });
  try {
    for (const value of [[], [1, 2, 3], {}, { nested: [true, null] }, new Uint8Array([0, 255]), undefined, null, 'text']) {
      let received = false;
      relay.addEventListener('message', event => {
        assert.deepEqual(event.detail, { sender: 2n, value });
        received = true;
      }, { once: true });
      WebSocketClass.instance.receive([2, value]);
      assert.ok(received);
      assert.equal(relay.ready, true);
    }
  } finally { relay.close(); }
});

test('malformed variable results reject before being exposed as found/value', async () => {
  for (const value of [new Uint8Array([1, 2]), 'ab', [], [true], [true, 1, 2],
    [1, null], ['true', null], [null, null], null, {}]) {
    const WebSocketClass = socketClass(1);
    const relay = await Relay.connect('ws://fixture', 'result-check', { WebSocketClass });
    try {
      const pending = relay.getVariable('key');
      await Promise.resolve();
      WebSocketClass.instance.receive([0, [200, value]]);
      await assert.rejects(pending, /Invalid variable response/);
      assert.equal(WebSocketClass.instance.closed.code, 4002);
    } finally { relay.close(); }
  }
});

test('boolean command replies reject nonboolean results', async () => {
  for (const op of [0, 4]) {
    for (const value of [0, 1, 'true', null, [], {}]) {
      const WebSocketClass = socketClass(1);
      const relay = await Relay.connect('ws://fixture', 'result-check', { WebSocketClass });
      try {
        const pending = relay.request(op, null);
        await Promise.resolve();
        WebSocketClass.instance.receive([0, [op, value]]);
        await assert.rejects(pending, /Invalid boolean response/);
        assert.equal(WebSocketClass.instance.closed.code, 4002);
      } finally { relay.close(); }
    }
  }
});

test('valid command results preserve false and arbitrary stored CBOR values', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'result-check', { WebSocketClass });
  try {
    for (const op of [0, 4]) {
      for (const value of [false, true]) {
        const pending = relay.request(op, null);
        await Promise.resolve();
        WebSocketClass.instance.receive([0, [op, value]]);
        assert.equal(await pending, value);
      }
    }
    for (const [found, value] of [[false, null], [true, undefined], [true, null],
      [true, new Uint8Array([0, 255])], [true, { nested: [false, 3] }]]) {
      const pending = relay.getVariable('key');
      await Promise.resolve();
      WebSocketClass.instance.receive([0, [200, [found, value]]]);
      assert.deepEqual(await pending, { found, value });
    }
    assert.equal(relay.ready, true);
  } finally { relay.close(); }
});

test('listener flags accept only effective integers in 0 through 7', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'listener-check', { WebSocketClass });
  try {
    for (const value of [0, 1, 7, 0n, 1n, 7n]) {
      const pending = relay.listen(1);
      await Promise.resolve();
      WebSocketClass.instance.receive([0, [1, value]]);
      assert.equal(await pending, value);
    }
  } finally { relay.close(); }
  for (const value of [-1, 8, 1.5, '1', true, false, null, [], {}, NaN, Infinity, -1n, 8n]) {
    const InvalidSocket = socketClass(1);
    const client = await Relay.connect('ws://fixture', 'listener-check', { WebSocketClass: InvalidSocket });
    try {
      const pending = client.listen(1);
      await Promise.resolve();
      InvalidSocket.instance.receive([0, [1, value]]);
      await assert.rejects(pending, /Invalid listener response/);
      assert.equal(InvalidSocket.instance.closed.code, 4002);
    } finally { client.close(); }
  }
});

test('channel listing rejects malformed rows and populations', async () => {
  const name = new Uint8Array([0, 255]);
  for (const value of ['list', {}, name, [1], [[name]], [[name, 1, 2]],
    [['text', 1]], [[new Uint8Array(17), 1]], [[name, -1]], [[name, 1.5]],
    [[name, NaN]], [[name, Infinity]], [[name, Number.MAX_SAFE_INTEGER + 1]],
    [[name, -1n]], [[name, BROADCAST + 1n]], [[name, '1']], [[name, true]],
    [[name, null]], [[name, undefined]], [[name, []]]]) {
    const WebSocketClass = socketClass(1);
    const relay = await Relay.connect('ws://fixture', 'listing-check', { WebSocketClass });
    try {
      const pending = relay.channels();
      await Promise.resolve();
      WebSocketClass.instance.receive([0, [2, value]]);
      await assert.rejects(pending, /Invalid channel listing response/);
      assert.equal(WebSocketClass.instance.closed.code, 4002);
    } finally { relay.close(); }
  }
});

test('channel listing preserves empty/binary names and exact numeric types', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'listing-check', { WebSocketClass });
  try {
    for (const rows of [[], [[new Uint8Array(), 0]], [[new Uint8Array([255, 0]), 1n]],
      [[new Uint8Array(16), Number.MAX_SAFE_INTEGER]], [[new Uint8Array([1]), BROADCAST]]]) {
      const pending = relay.channels();
      await Promise.resolve();
      WebSocketClass.instance.receive([0, [2, rows]]);
      assert.deepEqual(await pending, rows);
      assert.equal(relay.ready, true);
    }
  } finally { relay.close(); }
});

test('assignment acknowledgement updates identity before the next message callback', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'assignment-order', { WebSocketClass });
  try {
    const id = BROADCAST - 2n;
    const assignment = relay.assignId(id);
    await Promise.resolve();
    let observed;
    relay.addEventListener('message', event => {
      observed = { userId: relay.userId, detail: event.detail };
    }, { once: true });
    // A transport may dispatch consecutive decoded frames in one callback.
    WebSocketClass.instance.receive([0, [3, id]]);
    WebSocketClass.instance.receive([id, 'after assignment']);
    assert.deepEqual(observed, { userId: id, detail: { sender: id, value: 'after assignment' } });
    assert.equal(await assignment, id);
  } finally { relay.close(); }
});

test('low-level assignment acknowledgements update identity without changing result types', async () => {
  const WebSocketClass = socketClass(1);
  const relay = await Relay.connect('ws://fixture', 'assignment-order', { WebSocketClass });
  try {
    const pending = relay.request(3, 42n);
    await Promise.resolve();
    WebSocketClass.instance.receive([0, [3, 42]]);
    assert.equal(relay.userId, 42n);
    assert.equal(await pending, 42);
  } finally { relay.close(); }
});
