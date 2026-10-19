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
