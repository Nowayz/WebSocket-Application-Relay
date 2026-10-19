import assert from 'node:assert/strict';
import { once } from 'node:events';
import test from 'node:test';
import { Relay } from './relay.mjs';

test('original client assigns the forwarded first false reply to its next authentication', async t => {
  const failure = new Error('controlled original post-forward failure');
  class FailingSocket extends WebSocket {
    failNext = false;
    sends = 0;
    send(packet) {
      super.send(packet);
      this.sends++;
      if (this.failNext) { this.failNext = false; throw failure; }
    }
  }
  const relay = await Relay.connect(process.env.WAR_TEST_URL, 'js-old-fail', { WebSocketClass: FailingSocket });
  t.after(() => relay.close());
  const closed = once(relay, 'close');
  const before = relay.ws.sends;
  relay.ws.failNext = true;
  const results = await Promise.allSettled([relay.authenticate('wrong'), relay.authenticate('js-test-host')]);
  await closed;
  assert.equal(results[0].status, 'rejected');
  assert.equal(results[0].reason, failure);
  assert.equal(results[1].status, 'fulfilled');
  assert.equal(results[1].value, false);
  assert.equal(relay.ws.sends - before, 2);
  console.log(JSON.stringify({ event: 'original_reply_misassignment', original_error_preserved: true,
    queued_command: results[1], command_frames: relay.ws.sends - before }));
});
