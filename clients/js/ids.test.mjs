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
    assert.equal(WebSocketClass.instance.closed.code, 1002);
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
      assert.equal(WebSocketClass.instance.closed.code, 1002);
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
    assert.equal(WebSocketClass.instance.closed.code, 1002);
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
