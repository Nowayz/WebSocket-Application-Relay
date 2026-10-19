import assert from 'node:assert/strict';
import { once } from 'node:events';
import { Relay } from '../clients/js/relay.mjs';

const original = process.env.WAR_EXPECT_ORIGINAL === '1';
const holder = await Relay.connect(process.env.WAR_TEST_URL, 'full');
const outcomes = [];
try {
  for (let attempt = 0; attempt < 2; attempt++) {
    const socket = new WebSocket(process.env.WAR_TEST_BRIDGE_URL);
    socket.binaryType = 'arraybuffer';
    let messages = 0;
    socket.addEventListener('message', () => messages++);
    await once(socket, 'open');
    const closed = once(socket, 'close');
    socket.send('full');
    const [event] = await closed;
    assert.equal(messages, 0);
    assert.equal(event.code, original ? 1006 : 1013);
    assert.equal(event.reason, original ? '' : 'Try Again Later');
    assert.equal(event.wasClean, !original);
    outcomes.push({ attempt, code: event.code, reason: event.reason, clean: event.wasClean, messages });
  }
  const response = await fetch(process.env.WAR_TEST_ADMIN + '/metrics', {
    headers: { Authorization: 'Bearer bridge-welcome-fixture' },
  });
  assert.equal(response.status, 200);
  const metrics = Object.fromEntries((await response.text()).split('\n')
    .filter(line => line.startsWith('war_')).map(line => { const [key, value] = line.split(' '); return [key, Number(value)]; }));
  assert.equal(metrics.war_sessions, 1);
  assert.equal(metrics.war_connections, 1);
  assert.equal(metrics.war_accepted_total, 3);
  assert.equal(metrics.war_joined_total, 1);
  assert.equal(metrics.war_join_rejected_total, 2);
  assert.equal(metrics.war_rejected_total, 2);
  const closed = once(holder, 'close'); holder.close(); await closed;
  const socket = new WebSocket(process.env.WAR_TEST_BRIDGE_URL);
  socket.binaryType = 'arraybuffer';
  await once(socket, 'open');
  const welcome = once(socket, 'message'); socket.send('full');
  const bytes = new Uint8Array((await welcome)[0].data);
  assert.equal(bytes.length, 8);
  const packet = new Uint8Array(10); packet.set(bytes); packet.set([17, 34], 8);
  const received = once(socket, 'message'); socket.send(packet);
  assert.deepEqual(new Uint8Array((await received)[0].data), packet);
  const finished = once(socket, 'close'); socket.close(1000, 'done');
  const [event] = await finished;
  assert.equal(event.code, 1000); assert.equal(event.wasClean, true);
  console.log(JSON.stringify({ event: 'capacity_rejection', original, outcomes,
    native_rejections: metrics.war_join_rejected_total, replacement_routed: true }));
} finally {
  holder.close();
}
