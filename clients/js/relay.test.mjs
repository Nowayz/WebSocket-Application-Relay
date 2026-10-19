import assert from 'node:assert/strict';
import { once, getEventListeners } from 'node:events';
import test from 'node:test';
import { Relay, BROADCAST } from './relay.mjs';

const url = process.env.WAR_TEST_URL ?? 'ws://127.0.0.1:1338';

test('native listener acknowledgements return effective flags and private observation stays usable', async t => {
  const observer = await Relay.connect(url, 're_globl');
  const sender = await Relay.connect(url, 'js-listener');
  const receiver = await Relay.connect(url, 'js-listener');
  t.after(() => { observer.close(); sender.close(); receiver.close(); });
  assert.equal(await observer.authenticate('js-test-host'), true);
  for (const [flags, expected] of [[0, 0], [1, 1], [7, 7], [255, 7], [2, 2]]) {
    assert.equal(await observer.listen(flags), expected);
  }
  const observed = once(observer, 'message');
  const received = once(receiver, 'message');
  sender.send(receiver.userId, { listener: true });
  const expected = { sender: sender.userId, value: { listener: true } };
  assert.deepEqual((await observed)[0].detail, expected);
  assert.deepEqual((await received)[0].detail, expected);
  assert.equal(await observer.listen(0), 0);
  assert.equal(await observer.authenticate('js-test-host'), true);
});

test('CBOR client carries objects, bytes, text, uint64 IDs, and concurrent variable requests', async t => {
  const a = await Relay.connect(url, 'js-cbor');
  const b = await Relay.connect(url, 'js-cbor');
  t.after(() => { a.close(); b.close(); });
  const received = once(b, 'message');
  a.broadcast({ text: 'hello π', bytes: new Uint8Array([0, 255]), nested: [true, null, 42] });
  assert.deepEqual((await received)[0].detail, { sender: a.userId, value: { text: 'hello π', bytes: new Uint8Array([0, 255]), nested: [true, null, 42] } });
  await a.setVariable('typed', { phase: 3, ready: true });
  const results = await Promise.all([a.getVariable('typed'), a.getVariable('missing'), a.setVariable('empty', new Uint8Array()), a.getVariable('empty')]);
  assert.deepEqual(results[0], { found: true, value: { phase: 3, ready: true } });
  assert.deepEqual(results[1], { found: false, value: null });
  assert.deepEqual(results[3], { found: true, value: new Uint8Array() });
  assert.equal(await a.authenticate('js-test-host'), true);
  const fixed = 0xfedcba9876543210n;
  assert.equal(await a.assignId(fixed), fixed);
  const privateMessage = once(a, 'message');
  b.send(fixed, 'private text');
  assert.deepEqual((await privateMessage)[0].detail, { sender: b.userId, value: 'private text' });
  assert.throws(() => b.send(Number(BROADCAST), null), /BigInt/);
});

test('CBOR connections finish close handshakes before and after joining', async () => {
  for (const joined of [false, true]) {
    const ws = new WebSocket(url); ws.binaryType = 'arraybuffer';
    await once(ws, 'open');
    if (joined) {
      const welcome = once(ws, 'message');
      ws.send(new Uint8Array([0x65,0x63,0x6c,0x6f,0x73,0x65]));
      await welcome;
    }
    const closed = once(ws, 'close'); ws.close(4002, 'client finished');
    const event = (await closed)[0];
    assert.equal(event.code, 4002); assert.equal(event.reason, 'client finished');
    assert.equal(event.wasClean, true);
  }
});

test('application idle policy closes a CBOR client cleanly', { skip: !process.env.WAR_TEST_IDLE_ENABLED }, async t => {
  const relay = await Relay.connect(url, 'idle-native');
  const closed = once(relay.ws, 'close');
  t.after(() => relay.close());
  const [event] = await closed;
  assert.equal(event.code, 1001); assert.equal(event.reason, 'Application Idle');
  assert.equal(event.wasClean, true);
  assert.equal(getEventListeners(relay.ws, 'message').length, 0);
  assert.equal(getEventListeners(relay.ws, 'close').length, 0);
});

test('native channel listing returns sorted byte names and current populations', async t => {
  const binaryName = new Uint8Array([255, 0, 43, 47]);
  const host = await Relay.connect(url, binaryName);
  const member = await Relay.connect(url, binaryName);
  const empty = await Relay.connect(url, new Uint8Array());
  t.after(() => { host.close(); member.close(); empty.close(); });
  assert.equal(await host.authenticate('js-test-host'), true);
  const rows = await host.channels();
  const find = name => rows.find(([bytes]) => Buffer.from(bytes).equals(Buffer.from(name)));
  assert.equal(BigInt(find(binaryName)[1]), 2n);
  assert.equal(BigInt(find(new Uint8Array())[1]), 1n);
  assert.ok(find(new TextEncoder().encode('re_globl')));
  for (let i = 1; i < rows.length; i++) {
    assert.ok(Buffer.compare(rows[i - 1][0], rows[i][0]) < 0);
  }
  const received = once(host, 'message');
  host.send(host.userId, { afterListing: true });
  assert.deepEqual((await received)[0].detail.value, { afterListing: true });
});

test('native pipelined assignment and self-message expose the acknowledged identity', async t => {
  const relay = await Relay.connect(url, 'js-id-pipeline');
  t.after(() => relay.close());
  assert.equal(await relay.authenticate('js-test-host'), true);
  const id = 0xabcd000000001234n;
  const received = new Promise(resolve => relay.addEventListener('message', event => {
    resolve({ userId: relay.userId, detail: event.detail });
  }, { once: true }));
  const assignment = relay.assignId(id);
  // Start the queued control send, then pipeline data without awaiting its ack.
  await Promise.resolve();
  relay.send(id, { afterAssignment: true });
  const [assigned, observed] = await Promise.all([assignment, received]);
  assert.equal(assigned, id);
  assert.deepEqual(observed, { userId: id, detail: { sender: id, value: { afterAssignment: true } } });
});

test('native subscription retains the byte channel supplied before the socket opens', async t => {
  const expected = new Uint8Array([0, 255, 83, 78, 65, 80]);
  const channel = expected.slice();
  const pending = Relay.connect(url, channel);
  channel.fill(120);
  const relay = await pending;
  t.after(() => relay.close());
  assert.equal(await relay.authenticate('js-test-host'), true);
  const rows = await relay.channels();
  const row = rows.find(([name]) => name.length === expected.length
    && name.every((byte, index) => byte === expected[index]));
  assert.ok(row, 'subscription joined the originally supplied binary channel');
  assert.equal(row[1], 1);
});

test('native variable writes retain the value supplied before command dispatch', async t => {
  const relay = await Relay.connect(url, 'js-cmd-snapshot');
  t.after(() => relay.close());
  const value = { bytes: new Uint8Array([1, 2]), label: 'before' };
  const pending = relay.setVariable('snapshot', value);
  value.bytes.fill(9);
  value.label = 'after';
  assert.equal(await pending, true);
  assert.deepEqual(await relay.getVariable('snapshot'), {
    found: true, value: { bytes: new Uint8Array([1, 2]), label: 'before' },
  });
});

test('native connection remains usable after locally rejected reentrant serialization', async t => {
  const relay = await Relay.connect(url, 'js-reentry');
  t.after(() => relay.close());
  assert.throws(() => relay.send(relay.userId, { get child() {
    relay.send(relay.userId, 'nested');
    return 'outer';
  } }), /serialization.*reenter/i);
  const received = once(relay, 'message');
  relay.send(relay.userId, 'recovered');
  assert.deepEqual((await received)[0].detail, { sender: relay.userId, value: 'recovered' });
  assert.equal(relay.ready, true);
});


test('native final close detaches the live client socket handlers', async () => {
  const relay = await Relay.connect(url, 'js-final-close');
  const closed = once(relay, 'close');
  relay.close();
  const [event] = await closed;
  assert.equal(event.detail.code, 1000);
  assert.equal(getEventListeners(relay.ws, 'message').length, 0);
  assert.equal(getEventListeners(relay.ws, 'close').length, 0);
});

test('native command send failure cannot misassign a late authentication reply', async t => {
  const failure = new Error('controlled native post-forward failure');
  class FailingSocket extends WebSocket {
    failNext = false;
    sends = 0;
    send(bytes) {
      super.send(bytes);
      this.sends++;
      if (this.failNext) { this.failNext = false; throw failure; }
    }
  }
  const relay = await Relay.connect(url, 'js-send-failure', { WebSocketClass: FailingSocket });
  t.after(() => relay.close());
  const closed = once(relay, 'close');
  const before = relay.ws.sends;
  relay.ws.failNext = true;
  const settled = await Promise.allSettled([relay.authenticate('wrong'), relay.authenticate('js-test-host')]);
  const [event] = await closed;
  assert.equal(event.detail.code, 4002);
  assert.equal(event.detail.reason, 'Relay Command Send Failed');
  assert.equal(settled[0].status, 'rejected');
  assert.equal(settled[0].reason, failure);
  assert.equal(settled[1].status, 'rejected', 'late false reply resolved the next authentication');
  assert.match(settled[1].reason.message, /Relay is closed/);
  assert.equal(relay.ws.sends - before, 1, 'queued authentication reached the wire');
});
