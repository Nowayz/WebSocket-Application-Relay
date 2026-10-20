import { Relay, BROADCAST, LISTEN, type RelayEventMap } from '@war/relay-client';

async function example() {
  const relay = await Relay.connect('ws://localhost:1338', 'typed');
  const id: bigint = relay.userId;
  relay.send(id, { bytes: new Uint8Array([1, 2]), text: 'hello', count: 42n });
  relay.send(BROADCAST, [true, null, 1.5]);
  relay.addEventListener('message', event => {
    const sender: bigint = event.detail.sender;
    relay.send(sender, event.detail.value);
  });
  const listener = (event: RelayEventMap['message']) => relay.send(event.detail.sender, null);
  relay.addEventListener('message', listener);
  relay.removeEventListener('message', listener);
  relay.addEventListener('disconnect', event => {
    const departed: bigint = event.detail.userId;
    void departed;
  });
  await relay.listen(LISTEN.DISCONNECT);
  const variable = await relay.getVariable<{ score: number }>('state');
  if (variable.found) variable.value.score.toFixed();
  else { const missing: null = variable.value; void missing; }
  // @ts-expect-error Full uint64 IDs use bigint, not decimal strings.
  relay.send('18446744073709551615', null);
  // @ts-expect-error Channels are strings or bytes, not arbitrary objects.
  await Relay.connect('ws://localhost:1338', { channel: 'typed' });
}
void example;
