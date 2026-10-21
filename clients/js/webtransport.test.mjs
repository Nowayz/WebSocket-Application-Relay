import test from 'node:test';
import assert from 'node:assert/strict';
import { once } from 'node:events';
import { WebTransportChannel } from './webtransport.mjs';

class MockTransport {
  static last;
  writes = [];
  datagramWrites = [];
  closeCalls = [];
  ready = Promise.resolve();
  closed = new Promise(resolve => { this.finish = resolve; });
  constructor() {
    MockTransport.last = this;
    this.datagrams = {
      maxDatagramSize: 1200,
      readable: new ReadableStream({ start: controller => { this.datagramInput = controller; } }),
      createWritable: () => new WritableStream({ write: bytes => { this.datagramWrites.push(bytes); } }),
    };
  }
  async createBidirectionalStream() {
    return {
      readable: new ReadableStream({ start: controller => { this.input = controller; } }),
      writable: new WritableStream({ write: bytes => { this.writes.push(bytes); } }),
    };
  }
  close(info) { this.closeCalls.push(info); this.finish(info); this.datagramInput.close(); }
}
async function channel() {
  const channel = new WebTransportChannel('https://example.test/war', { WebTransportClass: MockTransport });
  await once(channel, 'open');
  return [channel, MockTransport.last];
}
const turn = () => new Promise(resolve => setImmediate(resolve));

test('partial and coalesced stream records retain boundaries and datagrams retain their lane', async () => {
  const [transport, mock] = await channel();
  const events = [];
  transport.addEventListener('message', event => events.push({ bytes: [...event.data], unreliable: event.unreliable }));
  mock.input.enqueue(Uint8Array.of(0, 0));
  mock.input.enqueue(Uint8Array.of(0, 2, 24));
  mock.input.enqueue(Uint8Array.of(42, 0, 0, 0, 1, 7));
  mock.datagramInput.enqueue(Uint8Array.of(9));
  await turn();
  assert.deepEqual(events.filter(e => !e.unreliable).map(e => e.bytes), [[24, 42], [7]]);
  assert.deepEqual(events.filter(e => e.unreliable).map(e => e.bytes), [[9]]);
  transport.close(4002, 'test end');
});

test('graceful close waits for the real stream acknowledgement and is idempotent', async () => {
  const [transport, mock] = await channel();
  const source = Uint8Array.of(3, 4);
  transport.send(source); source.fill(99);
  const closed = once(transport, 'close');
  transport.close(1000, 'finished'); transport.close(1000, 'finished');
  await turn();
  assert.deepEqual(mock.writes.map(bytes => [...bytes]), [[0,0,0,2,3,4], [0,0,0,0]]);
  assert.equal(mock.closeCalls.length, 0);
  mock.input.enqueue(new Uint8Array(4));
  const [event] = await closed;
  assert.equal(event.code, 1000);
  assert.equal(mock.closeCalls.length, 1);
});

test('truncated stream records fail and oversized datagrams are rejected before writing', async () => {
  const [transport, mock] = await channel();
  await assert.rejects(transport.sendUnreliable(new Uint8Array(1201)), RangeError);
  assert.equal(mock.datagramWrites.length, 0);
  const closed = new Promise(resolve => transport.addEventListener('close', resolve, { once: true }));
  mock.input.enqueue(Uint8Array.of(0, 0, 0, 2, 1));
  mock.input.close();
  assert.equal((await closed).code, 4002);
});
