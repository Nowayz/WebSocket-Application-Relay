import { Encoder } from 'cbor-x';
import { performance } from 'node:perf_hooks';
const { Relay } = await import(process.argv[2]);
const sender = 1n << 63n;
const bytes = new Encoder({ useRecords: false }).encode([sender, 'value']);
const event = new MessageEvent('message', { data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
const ws = new EventTarget(); ws.readyState = 1; ws.close = () => { throw new Error('unexpected close'); };
const relay = new Relay(ws, 1n, 1000);
let verified = 0;
relay.addEventListener('message', event => {
  if (event.detail.sender !== sender || event.detail.value !== 'value') throw new Error('bad payload');
  verified++;
});
for (let i = 0; i < 20000; i++) ws.dispatchEvent(event);
const ns = [];
for (let batch = 0; batch < 7; batch++) {
  const began = performance.now();
  for (let i = 0; i < 100000; i++) ws.dispatchEvent(event);
  ns.push((performance.now() - began) * 1e6 / 100000);
}
if (verified !== 720000) throw new Error('missing messages');
console.log(JSON.stringify({ verified, iterations_per_batch: 100000, ns_per_message: ns }));
