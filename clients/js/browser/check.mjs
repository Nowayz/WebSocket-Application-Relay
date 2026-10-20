// Copied beside the generated library, so checks use the deliverable bundle.
import { Relay, BROADCAST } from './relay.js';

const form = document.querySelector('#check');
const button = document.querySelector('#run');
const status = document.querySelector('#status');
const results = document.querySelector('#results');
const details = document.querySelector('#details');

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

function once(target, type) {
  return new Promise((resolve, reject) => {
    const listener = event => { clearTimeout(timer); resolve(event); };
    const timer = setTimeout(() => {
      target.removeEventListener(type, listener);
      reject(new Error(`Timed out waiting for ${type}`));
    }, 5_000);
    target.addEventListener(type, listener, { once: true });
  });
}

form.addEventListener('submit', async event => {
  event.preventDefault();
  button.disabled = true;
  results.replaceChildren();
  details.textContent = '';
  status.textContent = 'Running…';
  const report = { passed: false, url: document.querySelector('#url').value, checks: [], errors: [] };
  globalThis.warBrowserCheck = report;
  const clients = [];
  const pass = message => {
    report.checks.push(message);
    const item = document.createElement('li');
    item.textContent = message;
    results.append(item);
  };
  try {
    // Per-run names isolate concurrent browser checks and disappear on cleanup.
    const suffix = crypto.randomUUID().slice(0, 8);
    for (const channel of [`web-${suffix}`, `web-${suffix}`, `out-${suffix}`]) {
      const client = await Relay.connect(report.url, channel, { requestTimeoutMs: 5_000 });
      clients.push(client);
      client.addEventListener('error', ({ detail }) => report.errors.push(String(detail)));
      assert(typeof client.userId === 'bigint', 'Session ID lost its BigInt type');
      assert(client.ws.protocol === 'war.cbor.v1', 'CBOR subprotocol was not negotiated');
    }
    const [a, b, outsider] = clients;
    pass('Three browser WebSockets joined with BigInt IDs and war.cbor.v1.');

    const value = {
      text: 'Hello π / 世界', bytes: new Uint8Array([0, 1, 127, 255]),
      nested: [true, null, -42, 1.5], uint64: 0xfedcba9876543210n,
    };
    const broadcast = once(b, 'message');
    a.broadcast(value);
    const { detail: packet } = await broadcast;
    assert(packet.sender === a.userId && packet.value.text === value.text, 'Broadcast sender or text changed');
    assert(packet.value.bytes instanceof Uint8Array && packet.value.bytes.every((byte, i) => byte === value.bytes[i]) && packet.value.bytes.length === 4, 'Byte string changed');
    assert(packet.value.uint64 === value.uint64 && JSON.stringify(packet.value.nested) === JSON.stringify(value.nested), 'Typed values changed');
    pass('Broadcast preserved Unicode, byte strings, nested values, and an exact uint64.');

    const privateMessage = once(outsider, 'message');
    b.send(outsider.userId, 'cross-channel private');
    const privatePacket = (await privateMessage).detail;
    assert(privatePacket.sender === b.userId && privatePacket.value === 'cross-channel private', 'Private delivery changed');
    pass('Private routing reached a session in a different channel.');

    const bytes = new Uint8Array(131_072);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i % 251;
    const large = once(b, 'message');
    a.send(b.userId, bytes);
    const largePacket = (await large).detail;
    assert(largePacket.sender === a.userId && largePacket.value instanceof Uint8Array && largePacket.value.length === bytes.length && largePacket.value.every((byte, i) => byte === bytes[i]), 'Large binary message changed');
    pass('A 128 KiB binary value arrived byte for byte.');

    const image = new Uint8Array(5 * 1024 * 1024);
    for (let i = 0; i < image.length; i++) image[i] = i % 251;
    const file = once(b, 'message');
    a.send(b.userId, image);
    const filePacket = (await file).detail;
    assert(filePacket.sender === a.userId && filePacket.value instanceof Uint8Array && filePacket.value.length === image.length && filePacket.value.every((byte, i) => byte === image[i]), '5 MiB file message changed');
    pass('A 5 MiB file arrived byte for byte as one application message.');

    await a.setVariable('settings', { ready: true, bytes: new Uint8Array([3, 4]) });
    const values = await Promise.all([
      b.getVariable('settings'), b.getVariable('missing'),
      b.setVariable('empty', new Uint8Array()), b.getVariable('empty'),
    ]);
    assert(values[0].found && values[0].value.ready && values[0].value.bytes[1] === 4, 'Typed variable changed');
    assert(!values[1].found && values[1].value === null && values[3].found && values[3].value.length === 0, 'Missing or empty variable changed');
    pass('Concurrent control requests preserved typed, missing, and empty variables.');
    let rejected = false;
    try { a.send(Number(BROADCAST), null); } catch (error) { rejected = error instanceof RangeError; }
    assert(rejected, 'An imprecise Number target was accepted');
    pass('Unsafe Number IDs were rejected before sending.');

    const disconnect = once(a, 'disconnect');
    const closed = once(b.ws, 'close');
    b.close();
    assert((await disconnect).detail.userId === b.userId, 'Disconnect ID changed');
    const closedEvent = await closed;
    assert(closedEvent.wasClean && closedEvent.code === 1000, 'Peer Close handshake failed');
    for (const client of [a, outsider]) {
      const closed = once(client.ws, 'close');
      client.close();
      const event = await closed;
      assert(event.wasClean && event.code === 1000, 'Final Close handshake failed');
    }
    assert(report.errors.length === 0, report.errors.join('\n'));
    pass('Disconnect notification and all three clean Close handshakes completed.');
    report.passed = true;
    report.userAgent = navigator.userAgent;
    status.textContent = 'Passed';
  } catch (error) {
    report.errors.push(String(error));
    status.textContent = 'Failed';
  } finally {
    // Failure also releases sessions; never leave test data/variables behind.
    await Promise.allSettled(clients.filter(client => client.ws.readyState < 2).map(async client => {
      const closed = once(client.ws, 'close');
      client.close();
      await closed;
    }));
    details.textContent = JSON.stringify(report, null, 2);
    button.disabled = false;
    report.finished = true;
  }
});
