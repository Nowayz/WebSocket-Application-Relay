import assert from 'node:assert/strict';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';
import { Relay, BROADCAST } from './relay.mjs';

const url = process.env.WAR_TEST_URL ?? 'ws://127.0.0.1:1338';
const bridgeUrl = process.env.WAR_TEST_BRIDGE_URL ?? 'ws://127.0.0.1:1337';

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

test('the unchanged upstream relay.js works through the optional bridge', async t => {
  // Execute the actual 2018 script in a fresh realm, including its original prototype shim.
  const context = vm.createContext({ WebSocket, Uint8Array, ArrayBuffer, console, btoa, atob });
  vm.runInContext(await readFile(new URL('../../relay.js', import.meta.url), 'utf8') + '\nglobalThis.LegacyRelay = Relay; globalThis.legacyConstants = re;', context);
  const LegacyRelay = context.LegacyRelay;
  const ready = relay => new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('legacy join timed out')), 3000);
    relay.onSubscription = () => { clearTimeout(timer); resolve(); };
  });
  const a = new LegacyRelay(bridgeUrl, 'js-legacy');
  const b = new LegacyRelay(bridgeUrl, 'js-legacy');
  t.after(() => { a.ws.close(); b.ws.close(); });
  await Promise.all([ready(a), ready(b)]);
  const binary = new Promise(resolve => b.SetMessageHandler(0, 7, (sender, payload) => resolve({ sender, payload })));
  a.bSendTo(context.legacyConstants.BINARY_BROADCAST, 7, new Uint8Array([1, 2, 255]));
  const packet = await binary;
  assert.deepEqual(Array.from(packet.sender), Array.from(a.userId));
  assert.deepEqual(Array.from(new Uint8Array(packet.payload)), [1, 2, 255]);
  const text = new Promise(resolve => a.SetMessageHandler(1, 8, (sender, payload) => resolve({ sender, payload })));
  b.tSendTo(a.userId, { op: 8, text: 'original client' });
  assert.equal((await text).payload.text, 'original client');
  a.SetChannelVar('legacy-key', new Uint8Array([3, 4, 5]));
  const variable = await new Promise(resolve => a.GetChannelVar('legacy-key', resolve));
  assert.deepEqual(Array.from(variable), [3, 4, 5]);
  const disconnected = new Promise(resolve => b.onClientDisconnect = resolve);
  a.ws.close();
  assert.deepEqual(Array.from(await disconnected), Array.from(a.userId));
});

test('CBOR byte strings and text interoperate with legacy sessions through the bridge', async t => {
  const modern = await Relay.connect(url, 'bridge-types');
  const old = new WebSocket(bridgeUrl); old.binaryType = 'arraybuffer';
  t.after(() => { modern.close(); old.close(); });
  old.addEventListener('open', () => old.send('bridge-types'), { once: true });
  const [welcome] = await once(old, 'message');
  const idBytes = new Uint8Array(welcome.data);
  const oldId = new DataView(welcome.data).getBigUint64(0, true);
  const message = once(old, 'message');
  modern.send(oldId, new Uint8Array([7, 99]));
  const data = new Uint8Array((await message)[0].data);
  assert.equal(new DataView(data.buffer).getBigUint64(0, true), modern.userId);
  assert.deepEqual(Array.from(data.subarray(8)), [7, 99]);
  const modernMessage = once(modern, 'message');
  const binary = new Uint8Array(10); binary.set(idBytes); new DataView(binary.buffer).setBigUint64(0, modern.userId, true); binary.set([8, 100], 8);
  old.send(binary);
  assert.deepEqual((await modernMessage)[0].detail, { sender: oldId, value: new Uint8Array([8, 100]) });
});

test('legacy privileged commands preserve channel-list layout and fixed-ID takeover', async t => {
  const join = async channel => {
    const ws = new WebSocket(bridgeUrl); ws.binaryType = 'arraybuffer';
    await once(ws, 'open');
    const welcome = once(ws, 'message'); ws.send(channel);
    assert.equal((await welcome)[0].data.byteLength, 8);
    return ws;
  };
  const packet = (op, value = new Uint8Array()) => {
    const bytes = new Uint8Array(9 + value.length);
    bytes[8] = op; bytes.set(value, 9); return bytes;
  };
  const first = await join('legacy-host-a');
  const next = await join('legacy-host-b');
  t.after(() => { first.close(); next.close(); });
  for (const ws of [first, next]) ws.send(packet(0, new TextEncoder().encode('js-test-host')));
  const channelReply = once(first, 'message'); first.send(packet(2));
  const buffer = (await channelReply)[0].data;
  const view = new DataView(buffer);
  assert.equal(view.getBigUint64(0, true), 0n);
  const count = view.getUint32(8, true);
  let position = 12;
  const names = [];
  for (let i = 0; i < count; i++) {
    const length = view.getUint8(position++);
    names.push(new TextDecoder().decode(new Uint8Array(buffer, position, length)));
    position += length;
  }
  assert.equal(position + count * 4, buffer.byteLength);
  assert.equal(view.getUint32(position + names.indexOf('legacy-host-a') * 4, true), 1);
  assert.ok(names.includes('re_globl'));
  const fixed = new Uint8Array(8); new DataView(fixed.buffer).setBigUint64(0, 42n, true);
  first.send(packet(3, fixed));
  // A subsequent query is a FIFO barrier because legacy assignment has no ack.
  const barrier = once(first, 'message'); first.send(packet(2)); await barrier;
  first.send(packet(3, fixed));
  const sameId = once(first, 'message'); first.send(packet(2)); await sameId;
  const displaced = once(first, 'close'); next.send(packet(3, fixed));
  assert.equal((await displaced)[0].code, 4001);
  const current = once(next, 'message'); next.send(packet(2)); await current;
  const reserved = once(next, 'close'); next.send(packet(3, new Uint8Array(8)));
  assert.equal((await reserved)[0].code, 1008);
});

test('native CBOR and legacy connections finish close handshakes before and after joining', async () => {
  for (const [endpoint, joined] of [[url, false], [bridgeUrl, false], [url, true], [bridgeUrl, true]]) {
    const ws = new WebSocket(endpoint); ws.binaryType = 'arraybuffer';
    await once(ws, 'open');
    if (joined) {
      const welcome = once(ws, 'message');
      ws.send(endpoint === url ? new Uint8Array([0x65,0x63,0x6c,0x6f,0x73,0x65]) : 'close');
      await welcome;
    }
    const closed = once(ws, 'close');
    ws.close(4002, 'client finished');
    const event = (await closed)[0];
    assert.equal(event.code, 4002);
    assert.equal(event.reason, 'client finished');
    assert.equal(event.wasClean, true);
  }
});

test('an unsupported structured value closes a legacy recipient cleanly and leaves CBOR routing usable', async t => {
  const modern = await Relay.connect(url, 'typed-limit');
  const old = new WebSocket(bridgeUrl); old.binaryType = 'arraybuffer';
  t.after(() => { modern.close(); old.close(); });
  await once(old, 'open');
  const welcome = once(old, 'message'); old.send('typed-limit');
  const id = new DataView((await welcome)[0].data).getBigUint64(0, true);
  const closed = once(old, 'close');
  modern.send(id, { requiresCBOR: true });
  const event = (await closed)[0];
  assert.equal(event.code, 1003);
  assert.equal(event.wasClean, true);
  const message = once(modern, 'message');
  modern.send(modern.userId, { stillReady: true });
  assert.deepEqual((await message)[0].detail.value, { stillReady: true });
});

test('application idle policy reaches native and legacy browser WebSockets cleanly', { skip: !process.env.WAR_TEST_IDLE_ENABLED }, async t => {
  const native = await Relay.connect(url, 'idle-native');
  const nativeClosed = once(native.ws, 'close');
  const legacy = new WebSocket(bridgeUrl); legacy.binaryType = 'arraybuffer';
  const legacyClosed = once(legacy, 'close');
  t.after(() => { native.close(); legacy.close(); });
  await once(legacy, 'open');
  const welcome = once(legacy, 'message'); legacy.send('idle-legacy');
  assert.equal((await welcome)[0].data.byteLength, 8);
  for (const [event] of await Promise.all([nativeClosed, legacyClosed])) {
    assert.equal(event.code, 1001);
    assert.equal(event.reason, 'Application Idle');
    assert.equal(event.wasClean, true);
  }
});
