import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { once, getEventListeners } from 'node:events';
const source = await readFile(new URL('../target/browser/relay.js', import.meta.url));
const moduleUrl = `data:text/javascript;base64,${source.toString('base64')}`;
const nodeBuffer = globalThis.Buffer;
globalThis.Buffer = undefined;
let Relay;
try { ({ Relay } = await import(moduleUrl)); }
finally { globalThis.Buffer = nodeBuffer; }

test('built module captures a queued variable before mutation and transfer', async t => {
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-bundle-cmd');
  t.after(() => relay.close());
  const prior = relay.getVariable('absent');
  const value = { bytes: new Uint8Array([3, 4]), label: 'before' };
  const pending = relay.setVariable('snapshot', value);
  value.label = 'after';
  structuredClone(value.bytes.buffer, { transfer: [value.bytes.buffer] });
  await prior;
  assert.equal(await pending, true);
  assert.deepEqual(await relay.getVariable('snapshot'), {
    found: true, value: { bytes: new Uint8Array([3, 4]), label: 'before' },
  });
});


test('built module rejects nested sends and preserves the native connection', async t => {
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-built-reentry');
  t.after(() => relay.close());
  assert.throws(() => relay.send(relay.userId, { get child() {
    relay.send(relay.userId, 'nested');
    return 'outer';
  } }), /serialization.*reenter/i);
  const received = new Promise(resolve => relay.addEventListener('message', resolve, { once: true }));
  relay.send(relay.userId, 'recovered');
  assert.deepEqual((await received).detail, { sender: relay.userId, value: 'recovered' });
});


test('built module detaches its listeners after a locally requested close', async () => {
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-built-close');
  const closed = once(relay, 'close');
  relay.close();
  const [event] = await closed;
  assert.equal(event.detail.code, 1000);
  assert.equal(getEventListeners(relay.ws, 'message').length, 0);
  assert.equal(getEventListeners(relay.ws, 'close').length, 0);
});

test('built module detaches its listeners after server idle close', async () => {
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-built-idle');
  const [event] = await once(relay, 'close');
  assert.equal(event.detail.code, 1001);
  assert.equal(event.detail.reason, 'Application Idle');
  assert.equal(getEventListeners(relay.ws, 'message').length, 0);
  assert.equal(getEventListeners(relay.ws, 'close').length, 0);
});


test('built module rejects queued controls after a forwarded send throws', async t => {
  const failure = new Error('controlled built post-forward failure');
  class FailingSocket extends WebSocket {
    failNext = false;
    sends = 0;
    send(bytes) {
      super.send(bytes);
      this.sends++;
      if (this.failNext) { this.failNext = false; throw failure; }
    }
  }
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-built-fail', { WebSocketClass: FailingSocket });
  t.after(() => relay.close());
  const closed = once(relay, 'close');
  const before = relay.ws.sends;
  relay.ws.failNext = true;
  const results = await Promise.allSettled([relay.authenticate('wrong'), relay.authenticate('js-test-host')]);
  const [event] = await closed;
  assert.equal(event.detail.code, 4002);
  assert.equal(event.detail.reason, 'Relay Command Send Failed');
  assert.equal(results[0].status, 'rejected');
  assert.equal(results[0].reason, failure);
  assert.equal(results[1].status, 'rejected');
  assert.match(results[1].reason.message, /Relay is closed/);
  assert.equal(relay.ws.sends - before, 1);
});
